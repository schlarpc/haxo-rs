# The Haxophone's own packages. haxo-rs, haxo-web and haxo-bridge build
# from this repository's Rust workspace (`src`, see flake.nix); the rest are
# board tools packaged here.
{ crane, src }:
final: prev:
let
  craneLib = crane.mkLib final;
in
{
  haxo-rs = final.callPackage ./packages/haxo-rs { inherit craneLib src; };
  haxo-web = final.callPackage ./packages/haxo-web.nix { inherit craneLib src; };
  haxo-bridge = final.callPackage ./packages/haxo-bridge.nix { inherit craneLib src; };
  umtprd = final.callPackage ./packages/umtprd.nix { };
  haxophone-config = final.callPackage ./packages/haxophone-config { };
}
