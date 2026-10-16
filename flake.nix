{
  description = "The Haxophone: its firmware, and the NixOS that runs it on a Raspberry Pi Zero 2 W";

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
    # Rust builds (nix/packages/haxo-rs). crane vendors dependencies
    # straight from Cargo.lock, git dependencies included, so there is no
    # cargoHash to re-prefetch on every change.
    crane.url = "github:ipetkov/crane";
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

      # The Rust workspace and the files haxo001 installs, and nothing else:
      # the packages' source, so a change to the NixOS modules, the docs or
      # the flake rebuilds none of them.
      rustSrc = lib.fileset.toSource {
        root = ./.;
        fileset = lib.fileset.unions [
          ./Cargo.toml
          ./Cargo.lock
          ./build.rs
          ./src
          ./crates
          ./notemap.json
          ./midi/startup/Startup_Haxophone.mid
        ];
      };
    in
    {
      # haxo-rs, haxo-web, haxo-bridge, umtprd and haxophone-config.
      overlays.default = import ./nix/overlay.nix {
        inherit (inputs) crane;
        src = rustSrc;
      };

      nixosConfigurations.haxophone = lib.nixosSystem {
        modules = [ ./nix/nixos/configuration.nix ];
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

      packages = forAllSystems (
        pkgs:
        let
          haxoPkgs = pkgs.extend self.overlays.default;
        in
        {
          inherit (haxoPkgs)
            haxo-rs
            haxo-web
            haxo-bridge
            umtprd
            haxophone-config
            ;
          default = haxoPkgs.haxo-rs;
          mint-sd = haxoPkgs.callPackage ./nix/packages/mint-sd.nix {
            disko = inputs.disko.packages.${pkgs.stdenv.hostPlatform.system}.disko;
            diskoFile = ./nix/nixos/disko.nix;
            toplevel = haxophone.config.system.build.toplevel;
            hostName = haxophone.config.networking.hostName;
          };
        }
      );

      apps = forAllSystems (pkgs: {
        mint-sd = {
          type = "app";
          program = lib.getExe self.packages.${pkgs.stdenv.hostPlatform.system}.mint-sd;
        };
      });

      # cargo and the C libraries for the workspace, plus `deploy`.
      devShells = forAllSystems (pkgs: {
        default = pkgs.mkShell {
          inputsFrom = [ self.packages.${pkgs.stdenv.hostPlatform.system}.haxo-rs ];
          nativeBuildInputs = [
            (deployRsPackage pkgs)
          ];
        };
      });

      formatter = forAllSystems (pkgs: pkgs.nixfmt-tree);
    };
}
