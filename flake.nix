{
  description = "glor: configure Pixart-based Glorious mice (Model O 2 / I 2) on Linux";

  # flake.lock pins the exact revision, so this only decides where `nix flake update`
  # moves to. Tracking unstable keeps the toolchain current; consumers who want their own
  # nixpkgs should set `inputs.glor.inputs.nixpkgs.follows = "nixpkgs"`, which makes this
  # irrelevant to them. The package uses only long-stable nixpkgs API.
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      inherit (nixpkgs) lib;
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forAllSystems = f: lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in
    {
      packages = forAllSystems (pkgs: rec {
        glor = pkgs.callPackage ./package.nix { };
        default = glor;
      });

      # `nix run github:owner/glor -- info`
      apps = forAllSystems (pkgs: rec {
        glor = {
          type = "app";
          program = lib.getExe self.packages.${pkgs.stdenv.hostPlatform.system}.glor;
        };
        default = glor;
      });

      # For consumers who prefer an overlay to a flake input reference.
      overlays.default = final: _prev: {
        glor = final.callPackage ./package.nix { };
      };

      # NixOS: `programs.glor.enable = true;` installs the CLI and the udev rule, so the
      # device is usable without elevation.
      nixosModules.default =
        {
          config,
          pkgs,
          lib,
          ...
        }:
        let
          cfg = config.programs.glor;
        in
        {
          options.programs.glor = {
            enable = lib.mkEnableOption "glor, a configuration tool for Glorious mice";
            package = lib.mkOption {
              type = lib.types.package;
              default = self.packages.${pkgs.stdenv.hostPlatform.system}.glor;
              defaultText = lib.literalString "glor from this flake";
              description = "The glor package to use.";
            };
          };
          config = lib.mkIf cfg.enable {
            environment.systemPackages = [ cfg.package ];
            # Ships udev/70-glorious.rules, which tags the hidraw nodes with uaccess.
            services.udev.packages = [ cfg.package ];
          };
        };

      devShells = forAllSystems (pkgs: {
        default = pkgs.mkShell {
          inputsFrom = [ self.packages.${pkgs.stdenv.hostPlatform.system}.glor ];
          packages = with pkgs; [
            clippy
            rustfmt
            rust-analyzer
          ];
        };
      });

      # `nix flake check` builds the package and runs the test suite.
      checks = forAllSystems (pkgs: {
        inherit (self.packages.${pkgs.stdenv.hostPlatform.system}) glor;
        tests = self.packages.${pkgs.stdenv.hostPlatform.system}.glor.overrideAttrs (_: {
          pname = "glor-tests";
          doCheck = true;
        });
      });

      # `nix fmt` passes paths (defaulting to "."), and nixfmt does not recurse into
      # directories — pointing it straight at the package makes `nix fmt` fail on a bare
      # checkout. Collect the .nix files first.
      formatter = forAllSystems (
        pkgs:
        pkgs.writeShellApplication {
          name = "glor-fmt";
          runtimeInputs = [ pkgs.nixfmt-rfc-style ];
          text = ''
            find "''${@:-.}" -type f -name '*.nix' -not -path '*/.git/*' -print0 \
              | xargs -0 -r nixfmt
          '';
        }
      );
    };
}
