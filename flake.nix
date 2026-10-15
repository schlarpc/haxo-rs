{
  description = "NixOS for the Haxophone (Raspberry Pi Zero 2 W)";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-26.05";
    # The Raspberry Pi firmware-partition and config.txt modules.
    nixos-hardware.url = "github:NixOS/nixos-hardware/master";
    disko = {
      url = "github:nix-community/disko";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    preservation.url = "github:nix-community/preservation";
    deploy-rs.url = "github:serokell/deploy-rs";
    # Rust builds (packages/haxo-rs). crane vendors dependencies straight
    # from Cargo.lock, git dependencies included, so there is no cargoHash to
    # re-prefetch on every bump.
    crane.url = "github:ipetkov/crane";
    # The Haxophone firmware (docs/haxophone.md): our fork of
    # cardonabits/haxo-rs, where our changes are commits. Source-only, so
    # `nix flake update haxo-rs` picks up what's pushed to its main.
    haxo-rs = {
      url = "github:schlarpc/haxo-rs";
      flake = false;
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      deploy-rs,
      ...
    }@inputs:
    let
      lib = nixpkgs.lib;
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forAllSystems = f: lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});

      # deploy-rs's own package output builds against its pinned nixpkgs,
      # whose importCargoLock fetches crates from crates.io/api/v1 and now
      # 403s. Build the input's source with nixpkgs' rust toolchain instead.
      # Bump the vendor hash whenever `nix flake update deploy-rs` moves it.
      deployRsPackage =
        pkgs:
        pkgs.deploy-rs.overrideAttrs (old: {
          version = "0-unstable-${deploy-rs.lastModifiedDate}";
          src = deploy-rs;
          cargoDeps = pkgs.rustPlatform.fetchCargoVendor {
            src = deploy-rs;
            hash = "sha256-ONGMdmkKGPJ+6KF2hkZQBefkug/C5ZEqPidKR6OkCbU=";
          };
        });
      # deploy-rs.lib.<system>.activate.* embeds deploy-rs's package (the
      # remote `activate` helper). Re-instantiate the lib through the
      # upstream overlay with the package above substituted in, the recipe
      # from deploy-rs's README.
      deployLib =
        system:
        (import nixpkgs {
          inherit system;
          overlays = [
            deploy-rs.overlays.default
            (final: prev: {
              deploy-rs = {
                deploy-rs = deployRsPackage nixpkgs.legacyPackages.${system};
                lib = prev.deploy-rs.lib;
              };
            })
          ];
        }).deploy-rs.lib;

      haxophone = self.nixosConfigurations.haxophone;
    in
    {
      nixosConfigurations.haxophone = lib.nixosSystem {
        modules = [ ./nixos/configuration.nix ];
        specialArgs = { inherit inputs; };
      };

      # Over the USB gadget link. For large copies, Wi-Fi keeps them off the
      # stall-prone gadget: `deploy .#haxophone --hostname haxophone.local`.
      deploy.nodes.haxophone = {
        hostname = "10.68.0.1";
        sshUser = "haxophone";
        fastConnection = true;
        profiles.system = {
          user = "root";
          path = (deployLib "aarch64-linux").activate.nixos haxophone;
        };
      };

      checks = lib.genAttrs systems (system: (deployLib system).deployChecks self.deploy);

      packages = forAllSystems (pkgs: {
        haxo-rs = pkgs.callPackage ./packages/haxo-rs {
          craneLib = inputs.crane.mkLib pkgs;
          src = inputs.haxo-rs;
        };
        haxo-web = pkgs.callPackage ./packages/haxo-web.nix {
          craneLib = inputs.crane.mkLib pkgs;
          src = inputs.haxo-rs;
        };
        haxo-bridge = pkgs.callPackage ./packages/haxo-bridge.nix {
          craneLib = inputs.crane.mkLib pkgs;
          src = inputs.haxo-rs;
        };
        umtprd = pkgs.callPackage ./packages/umtprd.nix { };
        haxophone-config = pkgs.callPackage ./packages/haxophone-config { };
        mint-sd = pkgs.callPackage ./packages/mint-sd.nix {
          disko = inputs.disko.packages.${pkgs.stdenv.hostPlatform.system}.disko;
          diskoFile = ./nixos/disko.nix;
          toplevel = haxophone.config.system.build.toplevel;
          hostName = haxophone.config.networking.hostName;
          haxophone-config = self.packages.${pkgs.stdenv.hostPlatform.system}.haxophone-config;
        };
      });

      apps = forAllSystems (pkgs: {
        mint-sd = {
          type = "app";
          program = lib.getExe self.packages.${pkgs.stdenv.hostPlatform.system}.mint-sd;
        };
      });

      devShells = forAllSystems (pkgs: {
        default = pkgs.mkShellNoCC {
          nativeBuildInputs = [
            (deployRsPackage pkgs)
          ];
        };
      });

      formatter = forAllSystems (pkgs: pkgs.nixfmt-tree);
    };
}
