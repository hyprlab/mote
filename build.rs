//! Compile the app icon (data/mote.gresource.xml, or on macOS
//! data/mote.macos.gresource.xml) into a `.gresource` blob
//! that is embedded in the binary and registered at startup (see `main.rs`),
//! and copy the credits files for the About window to read.

fn main() {
    // data/CONTRIBUTORS and data/TRANSLATORS arrive with their first names
    // (tools/check-docs.py turns an empty one down); until then, nobody.
    // The whole of data/ is watched, so the files' arrival is noticed.
    println!("cargo:rerun-if-changed=data");
    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
    for name in ["CONTRIBUTORS", "TRANSLATORS"] {
        let text = std::fs::read_to_string(format!("data/{name}")).unwrap_or_default();
        std::fs::write(out.join(name), text).expect("writes the credits file");
    }
    // The build script runs on the host, so the target is asked for here
    // rather than with cfg!.
    let resources = if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        "data/mote.macos.gresource.xml"
    } else {
        "data/mote.gresource.xml"
    };
    glib_build_tools::compile_resources(&["data"], resources, "mote.gresource");
}
