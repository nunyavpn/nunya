use std::path::Path;

fn main() {
    // One proto definition serves both sides. It is not kept here: scripts/fetch-core.sh installs
    // the exact file published with the pinned nunya-core release, after checking it against that
    // release's SHA256SUMS. Vendoring a copy instead would let the two drift silently, and editing
    // one here would produce a client that disagrees with the core it ships against.
    let proto_dir = Path::new("../vendor/core/proto");
    let proto = proto_dir.join("nunya.proto");

    assert!(
        proto.exists(),
        "{} not found — run ./scripts/fetch-core.sh to install the pinned core",
        proto.display()
    );

    // A release bundle is what `grant_root` makes setuid root, and a core built from source has its
    // parent check compiled out — the only thing that keeps other programs from driving it. So a
    // release build refuses one here, for every path to a bundle, not just `build-app.sh`.
    let origin = Path::new("../vendor/core/.origin");
    println!("cargo:rerun-if-changed={}", origin.display());
    if std::env::var("PROFILE").as_deref() == Ok("release")
        && std::fs::read_to_string(origin).is_ok_and(|o| o.trim() == "source")
    {
        panic!(
            "vendor/core holds a development core, built from source with its parent check \
             disabled; a release build must not bundle one. Run ./scripts/fetch-core.sh to \
             install the pinned release."
        );
    }

    prost_build::compile_protos(&[&proto], &[proto_dir])
        .expect("failed to compile nunya.proto (is protoc installed?)");

    println!("cargo:rerun-if-changed={}", proto.display());

    build_macos_tunnel_manager();

    tauri_build::build();
}

/// Compiles the NETunnelProviderManager wrapper and links the frameworks it needs.
///
/// This is the app's *control plane* for the VPN profile — saving it, starting it, talking to the
/// provider. It is not the tunnel itself: that lives in the packet tunnel extension, which the
/// system launches.
#[cfg(target_os = "macos")]
fn build_macos_tunnel_manager() {
    const SOURCE: &str = "macos/tunnel_manager.m";

    println!("cargo:rerun-if-changed={SOURCE}");
    println!("cargo:rerun-if-changed=macos/tunnel_manager.h");

    cc::Build::new()
        .file(SOURCE)
        .flag("-fobjc-arc")
        // NetworkExtension's control APIs predate this by far; the bundle targets 12.0.
        .flag("-mmacosx-version-min=12.0")
        .compile("nunya_tunnel_manager");

    println!("cargo:rustc-link-lib=framework=Foundation");
    println!("cargo:rustc-link-lib=framework=NetworkExtension");
}

#[cfg(not(target_os = "macos"))]
fn build_macos_tunnel_manager() {}
