# The tool behind the settings file on the FAT partition
# (config/haxophone.toml): applies it at boot, and checks it for mint-sd.
{
  lib,
  runCommand,
  python3,
}:
runCommand "haxophone-config" { meta.mainProgram = "haxophone-config"; } ''
  install -Dm644 ${./haxophone.toml} $out/share/haxophone.toml
  install -Dm755 ${./haxophone-config.py} $out/bin/haxophone-config
  sed -i '1i #!${lib.getExe python3}' $out/bin/haxophone-config
''
