//! `mdif-watch` is `mdif` with the Windows GUI subsystem: started at login it
//! opens no console window. The same source, so the linker is told, for that
//! one program only.

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc")
    {
        // The console entry point, `main`, is kept by naming it.
        println!("cargo::rustc-link-arg-bin=mdif-watch=/SUBSYSTEM:WINDOWS");
        println!("cargo::rustc-link-arg-bin=mdif-watch=/ENTRY:mainCRTStartup");
    }
}
