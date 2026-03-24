use crate::{
    EthCheatCtx, EthInspectorExt,
    backend::{DatabaseExt, JournaledState},
};
use alloy_evm::{EthEvm, Evm, EvmEnv, eth::EthEvmBuilder, precompiles::PrecompilesMap};
use foundry_fork_db::DatabaseError;
use revm::{
    context::{
        BlockEnv, Cfg, ContextTr, LocalContextTr, TxEnv,
        result::{EVMError, HaltReason, ResultAndState},
    },
    handler::{EvmTr, FrameResult, Handler, MainnetHandler},
    inspector::InspectorHandler,
    interpreter::{FrameInput, SharedMemory, interpreter_action::FrameInit},
    primitives::hardfork::SpecId,
};

pub fn new_eth_evm_with_inspector<I: EthInspectorExt>(
    db: &mut dyn DatabaseExt,
    evm_env: EvmEnv,
    inspector: I,
) -> EthEvm<&mut dyn DatabaseExt, I, PrecompilesMap> {
    let mut evm = EthEvmBuilder::new(db, evm_env).activate_inspector(inspector).build();
    evm.ctx_mut().cfg.tx_chain_id_check = true;

    let (_, insp, precompiles) = evm.components_mut();
    insp.get_networks().inject_precompiles(precompiles);
    evm
}

/// Object-safe trait exposing the operations that cheatcode nested EVM closures need.
///
/// This abstracts over the concrete EVM type (`EthEvm`, future `TempoEvm`, etc.)
/// so that cheatcode impls can build and run nested EVMs without knowing the concrete type.
pub trait NestedEvm {
    /// The transaction environment type.
    type Tx;
    /// The block environment type.
    type Block;
    /// The EVM spec (hardfork) type.
    type Spec;

    /// Returns a mutable reference to the journal inner state (`JournaledState`).
    fn journal_inner_mut(&mut self) -> &mut JournaledState;

    /// Runs a single execution frame (create or call) through the EVM handler loop.
    fn run_execution(&mut self, frame: FrameInput) -> Result<FrameResult, EVMError<DatabaseError>>;

    /// Executes a full transaction with the given tx env.
    fn transact(
        &mut self,
        tx: Self::Tx,
    ) -> Result<ResultAndState<HaltReason>, EVMError<DatabaseError>>;

    /// Returns a snapshot of the current EVM environment (cfg + block).
    fn to_evm_env(self) -> EvmEnv<Self::Spec, Self::Block>;
}

impl<I: EthInspectorExt> NestedEvm for EthEvm<&'_ mut dyn DatabaseExt, I, PrecompilesMap> {
    type Tx = TxEnv;
    type Block = BlockEnv;
    type Spec = SpecId;

    fn journal_inner_mut(&mut self) -> &mut JournaledState {
        &mut self.ctx_mut().journaled_state.inner
    }

    fn run_execution(&mut self, frame: FrameInput) -> Result<FrameResult, EVMError<DatabaseError>> {
        let inner = self.inner_mut();

        let mut handler: MainnetHandler<_, EVMError<DatabaseError>, _> = MainnetHandler::default();

        // Create first frame
        let memory =
            SharedMemory::new_with_buffer(inner.ctx().local().shared_memory_buffer().clone());
        let first_frame_input = FrameInit { depth: 0, memory, frame_input: frame };

        // Run execution loop
        let mut frame_result = handler.inspect_run_exec_loop(inner, first_frame_input)?;

        // Handle last frame result
        handler.last_frame_result(inner, &mut frame_result)?;

        Ok(frame_result)
    }

    fn transact(
        &mut self,
        tx: TxEnv,
    ) -> Result<ResultAndState<HaltReason>, EVMError<DatabaseError>> {
        Evm::transact_raw(self, tx)
    }

    fn to_evm_env(self) -> EvmEnv {
        Evm::finish(self).1
    }
}

/// Closure type used by `CheatcodesExecutor` methods that run nested EVM operations.
pub type NestedEvmClosure<'a, Block, Tx, Spec> =
    &'a mut dyn FnMut(
        &mut dyn NestedEvm<Block = Block, Tx = Tx, Spec = Spec>,
    ) -> Result<(), EVMError<DatabaseError>>;

/// Clones the current context (env + journal), passes the database, cloned env,
/// and cloned journal inner to the callback. The callback builds whatever EVM it
/// needs, runs its operations, and returns `(result, modified_env, modified_journal)`.
/// Modified state is written back after the callback returns.
pub fn with_cloned_context<CTX: EthCheatCtx>(
    ecx: &mut CTX,
    f: impl FnOnce(
        &mut dyn DatabaseExt<CTX::Block, CTX::Tx, <CTX::Cfg as Cfg>::Spec>,
        EvmEnv<<CTX::Cfg as Cfg>::Spec, CTX::Block>,
        JournaledState,
    ) -> Result<
        (EvmEnv<<CTX::Cfg as Cfg>::Spec, CTX::Block>, JournaledState),
        EVMError<DatabaseError>,
    >,
) -> Result<(), EVMError<DatabaseError>> {
    let evm_env = ecx.evm_clone();

    let (db, journal_inner) = ecx.db_journal_inner_mut();
    let journal_inner_clone = journal_inner.clone();

    let (sub_evm_env, sub_inner) = f(db, evm_env, journal_inner_clone)?;

    // Write back modified state. The db borrow was released when f returned.
    ecx.set_journal_inner(sub_inner);
    ecx.set_evm(sub_evm_env);

    Ok(())
}
