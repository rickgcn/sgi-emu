//! Reusable device models for the emulator.

mod common;

pub mod bus;
pub mod chipset;
pub mod cpu;
pub mod input;
pub mod memory;
pub mod parallel;
pub mod rtc;
pub mod serial;
pub mod state;
macro_rules! component_state {
    ($state:ident, $component:ty) => {
        #[doc = "Serializable deterministic component state."]
        #[derive(Clone, serde::Deserialize, serde::Serialize)]
        pub struct $state($component);

        impl $component {
            #[doc = "Captures all hardware-visible and in-flight component state."]
            pub fn save_state(&self) -> $state {
                $state(self.clone())
            }

            #[doc = "Restores validated component state without changing topology identity."]
            pub fn restore_state(
                &mut self,
                state: $state,
            ) -> Result<(), crate::state::DeviceStateError> {
                let expected = se_core::component::Component::id(self);
                let actual = se_core::component::Component::id(&state.0);
                if actual != expected {
                    return Err(crate::state::DeviceStateError::ComponentIdMismatch {
                        expected,
                        actual,
                    });
                }
                *self = state.0;
                Ok(())
            }
        }
    };
}

pub(crate) use component_state;
