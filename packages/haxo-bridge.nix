# haxo-bridge: haxo001's audio tap into an ALSA device on another clock (the
# USB audio gadget, or PipeWire for Bluetooth). A binary in the haxo-rs
# workspace (crates/haxo-bridge).
#
# `src` is the `haxo-rs` flake input, as for packages/haxo-rs.
{
  lib,
  craneLib,
  src,
  pkg-config,
  alsa-lib,
}:
let
  commonArgs = {
    pname = "haxo-bridge";
    version = "0-unstable-${src.lastModifiedDate or "unknown"}";
    inherit src;
    strictDeps = true;
    cargoExtraArgs = "--locked -p haxo-bridge";
    nativeBuildInputs = [ pkg-config ];
    buildInputs = [ alsa-lib ];
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
      description = "Plays the Haxophone's audio tap into another sound device";
      platforms = lib.platforms.linux;
      mainProgram = "haxo-bridge";
    };
  }
)
