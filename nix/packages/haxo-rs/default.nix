# haxo001, the Haxophone's firmware: this repository's root crate (our fork
# of cardonabits/haxo-rs; see docs/haxophone.md "Patches to make").
#
# `src` is the Rust workspace alone (flake.nix), so a change elsewhere in
# the repository, the NixOS modules or the docs, rebuilds nothing. crane
# vendors from its Cargo.lock; reading that from the flake's own source is
# not import-from-derivation.
{
  lib,
  craneLib,
  src,
  pkg-config,
  alsa-lib,
  fluidsynth,
  dbus,
}:
let
  # The source's store hash stands in for the commit, which would change
  # with every commit to the repository, Rust or not.
  srcHash = builtins.unsafeDiscardStringContext (builtins.substring 0 32 (baseNameOf (toString src)));
  commonArgs = {
    pname = "haxo-rs";
    version = "0-unstable-${builtins.substring 0 7 srcHash}";
    inherit src;
    strictDeps = true;

    # MIDI out to the USB gadget, and Bluetooth MIDI (`ble`, which pulls in
    # `midi`). `instrumentation` stays off: it toggles UART RXD, which is
    # the serial console.
    cargoExtraArgs = "--locked --features ble";

    # build.rs asks vergen for git metadata; the source has no .git, so
    # vergen falls back to placeholders unless these are set. `--version`
    # prints the describe string: the source hash, as in the version.
    VERGEN_GIT_DESCRIBE = "src-${builtins.substring 0 7 srcHash}";
    VERGEN_GIT_SHA = srcHash;

    nativeBuildInputs = [ pkg-config ];
    buildInputs = [
      alsa-lib
      fluidsynth
      dbus
    ];

    # The tests drive the real GPIO, I2C and ALSA hardware.
    doCheck = false;
  };
in
craneLib.buildPackage (
  commonArgs
  // {
    # The dependencies depend on Cargo.toml and Cargo.lock only. Without
    # the source hash (in the version and vergen's), every change to the
    # Rust source would rebuild all of them under emulation.
    cargoArtifacts = craneLib.buildDepsOnly (
      builtins.removeAttrs commonArgs [
        "VERGEN_GIT_DESCRIBE"
        "VERGEN_GIT_SHA"
      ]
      // {
        version = "0";
      }
    );

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
