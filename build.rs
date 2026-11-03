//! Compiles translations, embeds Windows resources.

fn main() {
    #[cfg(feature = "soloist-native-audio")]
    native_soloist_audio();
    fastframe_i18n::build::compile_catalogs("assets/i18n");
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rerun-if-changed=packaging/windows/spotifast.ico");
        let mut resource = winresource::WindowsResource::new();
        resource
            .set_icon("packaging/windows/spotifast.ico")
            .set("ProductName", "Spotifast")
            // Task Manager and the Windows mixer name the process by its description.
            .set("FileDescription", "Spotifast");
        if let Err(error) = resource.compile() {
            println!("cargo:warning=Windows resources not embedded: {error}");
        }
    }
}

#[cfg(feature = "soloist-native-audio")]
fn native_soloist_audio() {
    let target = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let source = match target.as_str() {
        "macos" => "src/soloist/native/macos.mm",
        "windows" => "src/soloist/native/windows.cpp",
        _ => return,
    };
    println!("cargo:rerun-if-changed={source}");
    println!("cargo:rerun-if-changed=src/soloist/native/audio.h");
    let mut build = cc::Build::new();
    build.cpp(true).std("c++17").file(source);
    if target == "macos" {
        build.flag("-fobjc-arc");
        for framework in ["Foundation", "CoreAudio", "AudioToolbox"] {
            println!("cargo:rustc-link-lib=framework={framework}");
        }
    } else {
        build.flag_if_supported("/EHsc");
        println!("cargo:rustc-link-lib=mmdevapi");
        println!("cargo:rustc-link-lib=ole32");
        println!("cargo:rustc-link-lib=uuid");
    }
    build.compile("spotifast_soloist_audio");
}
