# haxo-web, the web UI's server: a second binary in the haxo-rs workspace
# (crates/haxo-web, with the page in ui/). Pure Rust, so no C libraries.
#
# `src` is the `haxo-rs` flake input, as for packages/haxo-rs.
{
  lib,
  craneLib,
  src,
}:
let
  commonArgs = {
    pname = "haxo-web";
    version = "0-unstable-${src.lastModifiedDate or "unknown"}";
    inherit src;
    strictDeps = true;
    cargoExtraArgs = "--locked -p haxo-web";
    # Its tests run on any machine (cargo test -p haxo-web); skipped here
    # to keep the emulated aarch64 build short.
    doCheck = false;
  };
in
craneLib.buildPackage (
  commonArgs
  // {
    # Keyed on Cargo.toml and Cargo.lock only, not the commit (see
    # packages/haxo-rs).
    cargoArtifacts = craneLib.buildDepsOnly (commonArgs // { version = "0"; });
    meta = {
      description = "The Haxophone's web UI";
      platforms = lib.platforms.linux;
      mainProgram = "haxo-web";
    };
  }
)
