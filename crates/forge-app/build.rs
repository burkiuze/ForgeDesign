//! Embeds the application icon into the Windows executable.
fn main() {
    let target = std::env::var("TARGET").unwrap_or_default();
    if !target.contains("windows") {
        return;
    }
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let ico = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packaging/windows/forge.ico");
    let rc = out.join("forge.rc");
    std::fs::write(&rc, format!("1 ICON \"{}\"\n", ico.display().to_string().replace('\\', "/"))).unwrap();
    let res = out.join("forge-res.o");
    let windres = if target.contains("gnu") && !cfg!(windows) { "x86_64-w64-mingw32-windres" } else { "windres" };
    let ok = std::process::Command::new(windres).arg(&rc).arg("-O").arg("coff").arg("-o").arg(&res).status().map(|s| s.success()).unwrap_or(false);
    if ok {
        println!("cargo:rustc-link-arg-bins={}", res.display());
    }
    println!("cargo:rerun-if-changed={}", ico.display());
}
