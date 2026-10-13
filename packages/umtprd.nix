# uMTP-Responder: an MTP device-side daemon for Linux USB gadgets
# (FunctionFS). Built with SYSTEMD=1 so it sends READY=1 once it has written
# its USB descriptors, which is when the gadget may be bound to the UDC; see
# nixos/gadget.nix.
{
  lib,
  stdenv,
  fetchFromGitHub,
  systemdLibs,
}:
stdenv.mkDerivation (finalAttrs: {
  pname = "umtprd";
  version = "1.8.1";

  src = fetchFromGitHub {
    owner = "viveris";
    repo = "uMTP-Responder";
    tag = "umtprd-${finalAttrs.version}";
    hash = "sha256-kZXuEgyxNHKbuZjoMpOVjt6ygiar73/C1FsF942pjFM=";
  };

  buildInputs = [ systemdLibs ];

  makeFlags = [
    "SYSTEMD=1"
    "CC=${stdenv.cc.targetPrefix}cc"
  ];

  installPhase = ''
    runHook preInstall
    install -Dm755 umtprd $out/bin/umtprd
    install -Dm644 conf/umtprd.conf $out/share/doc/umtprd/umtprd.conf
    runHook postInstall
  '';

  meta = {
    description = "Lightweight USB Media Transfer Protocol (MTP) responder daemon";
    homepage = "https://github.com/viveris/uMTP-Responder";
    license = lib.licenses.gpl3Plus;
    platforms = lib.platforms.linux;
    mainProgram = "umtprd";
  };
})
