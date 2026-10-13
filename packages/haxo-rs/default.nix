# haxo-rs, the Haxophone's firmware, built from our fork
# (schlarpc/haxo-rs, forked from cardonabits/haxo-rs). Our changes are
# commits there; see docs/haxophone.md "Patches to make".
#
# `src` is the `haxo-rs` flake input. crane vendors from the fork's
# Cargo.lock; reading it from a flake input is not import-from-derivation.
{
  lib,
  craneLib,
  src,
  pkg-config,
  alsa-lib,
  fluidsynth,
}:
let
  commonArgs = {
    pname = "haxo-rs";
    version = "0-unstable-${src.lastModifiedDate or "unknown"}";
    inherit src;
    strictDeps = true;

    # MIDI out to the USB gadget. `instrumentation` stays off: it toggles
    # UART RXD, which is the serial console.
    cargoExtraArgs = "--locked --features midi";

    # build.rs asks vergen for git metadata; the source has no .git, so
    # vergen falls back to placeholders unless these are set. `--version`
    # prints the describe string.
    VERGEN_GIT_DESCRIBE = builtins.substring 0 7 (src.rev or "unknown");
    VERGEN_GIT_SHA = src.rev or "unknown";

    nativeBuildInputs = [ pkg-config ];
    buildInputs = [
      alsa-lib
      fluidsynth
    ];

    # The tests drive the real GPIO, I2C and ALSA hardware.
    doCheck = false;
  };
in
craneLib.buildPackage (
  commonArgs
  // {
    cargoArtifacts = craneLib.buildDepsOnly commonArgs;

    postInstall = ''
      install -Dm644 notemap.json $out/share/haxo/notemap.json
      install -Dm644 midi/startup/Startup_Haxophone.mid $out/share/haxo/Startup_Haxophone.mid
    '';

    meta = {
      description = "Software for the Haxophone digital saxophone";
      homepage = "https://github.com/cardonabits/haxo-rs";
      license = lib.licenses.mit;
      platforms = lib.platforms.linux;
      mainProgram = "haxo001";
    };
  }
)
