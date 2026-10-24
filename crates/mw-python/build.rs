//! An extension module leaves Python's symbols to the interpreter that
//! loads it. Linux's linker allows that for a shared library; macOS's must
//! be told, as `pyo3_build_config::add_extension_module_link_args` tells
//! it. maturin passes the same arguments itself; this is for builds by
//! hand (docs/design/scripting.md §5.5).

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let macos = std::env::var("CARGO_CFG_TARGET_OS").is_ok_and(|os| os == "macos");
    if macos && std::env::var_os("CARGO_FEATURE_PYTHON").is_some() {
        println!("cargo:rustc-cdylib-link-arg=-undefined");
        println!("cargo:rustc-cdylib-link-arg=dynamic_lookup");
    }
}
