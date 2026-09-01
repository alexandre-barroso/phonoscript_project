use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=windows/convalgen.rc");
    println!("cargo:rerun-if-changed=assets/icon/windows/ConvalGEN.ico");

    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let resource_script = manifest_dir.join("windows/convalgen.rc");
    let target_env = env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();

    let resource = if target_env == "msvc" {
        let output = out_dir.join("convalgen.res");
        let compiler = env::var_os("RC").unwrap_or_else(|| "rc.exe".into());
        let status = Command::new(compiler)
            .current_dir(&manifest_dir)
            .arg("/nologo")
            .arg(format!("/fo{}", output.display()))
            .arg(&resource_script)
            .status()
            .expect("failed to launch the Windows resource compiler (rc.exe)");
        assert!(
            status.success(),
            "rc.exe failed to compile the ConvalGEN icon"
        );
        output
    } else {
        let output = out_dir.join("convalgen-icon.o");
        let compiler = env::var_os("WINDRES").unwrap_or_else(|| "windres".into());
        let status = Command::new(compiler)
            .current_dir(&manifest_dir)
            .arg(&resource_script)
            .arg("--output-format=coff")
            .arg("--output")
            .arg(&output)
            .status()
            .expect("failed to launch the Windows GNU resource compiler (windres)");
        assert!(
            status.success(),
            "windres failed to compile the ConvalGEN icon"
        );
        output
    };

    println!("cargo:rustc-link-arg-bin=convalgen={}", resource.display());
}
