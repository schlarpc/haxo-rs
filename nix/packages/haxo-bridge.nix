# haxo-bridge: haxo001's audio tap into an ALSA device on another clock (the
# USB audio gadget, or PipeWire for Bluetooth). A binary in the
# workspace (crates/haxo-bridge).
#
# `src` is the Rust workspace, as for nix/packages/haxo-rs.
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
    version = "0-unstable-${
      builtins.unsafeDiscardStringContext (builtins.substring 0 7 (baseNameOf (toString src)))
    }";
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
    # Keyed on Cargo.toml and Cargo.lock only, not the source (see
    # nix/packages/haxo-rs).
    cargoArtifacts = craneLib.buildDepsOnly (commonArgs // { version = "0"; });
    meta = {
      description = "Plays the Haxophone's audio tap into another sound device";
      platforms = lib.platforms.linux;
      mainProgram = "haxo-bridge";
    };
  }
)
