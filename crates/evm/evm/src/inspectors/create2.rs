use alloy_primitives::Address;

/// Inspector-level CREATE2 factory interception.
///
/// Will replace `FoundryHandler`'s frame-level CREATE2 override once
/// [revm#3518](https://github.com/bluealloy/revm/pull/3518) lands, adding
/// `frame_start`/`frame_end` callbacks to the `Inspector` trait.
///
/// For now this struct holds the deployer address and override tracking state,
/// synced via [`InspectorStackInner::set_create2_deployer`].
#[derive(Clone, Debug, Default)]
pub struct Create2Inspector {
    /// The CREATE2 deployer address.
    deployer: Address,
}

impl Create2Inspector {
    /// Set the CREATE2 deployer address.
    pub fn set_deployer(&mut self, deployer: Address) {
        self.deployer = deployer;
    }

    /// Returns the CREATE2 deployer address.
    pub fn deployer(&self) -> Address {
        self.deployer
    }
}
