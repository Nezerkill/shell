#![allow(non_camel_case_types)] // abi_stable's `_Ref` naming convention

//! R7 spike: the whole ABI surface is two function pointers.
//!
//! Deliberately tiny (see docs/architecture/0001-foundations.md §6.1): one
//! version function and one byte-oriented `call`. Everything else in the real
//! SDK (`Plugin` sabi_trait, `HostApi`, manifest, init/shutdown) is left out.

use abi_stable::std_types::{RResult, RSlice, RString, RVec};
use abi_stable::StableAbi;

/// ABI version the host expects. Any other value means "refuse to load".
pub const HOST_PLUGIN_API: u32 = 1;

/// Plugin entry point. `plugin_api` is checked by the host before anything
/// else is called; `call(method, payload)` is the only real entry point.
#[repr(C)]
#[derive(StableAbi)]
#[sabi(kind(Prefix(prefix_ref = RootModule_Ref)))]
#[sabi(missing_field(panic))]
pub struct RootModule {
    pub plugin_api: extern "C" fn() -> u32,
    #[sabi(last_prefix_field)]
    pub call: extern "C" fn(method: u32, payload: RSlice<'_, u8>) -> RResult<RVec<u8>, RString>,
}

// Without this `RootModule_Ref` cannot be loaded by abi_stable's loader and
// `export_root_module!` refuses to compile.
impl abi_stable::library::RootModule for RootModule_Ref {
    abi_stable::declare_root_module_statics! {RootModule_Ref}
    const BASE_NAME: &'static str = "r7_plugin";
    const NAME: &'static str = "r7_plugin";
    const VERSION_STRINGS: abi_stable::sabi_types::VersionStrings =
        abi_stable::package_version_strings!();
}
