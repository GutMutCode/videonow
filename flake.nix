{
  description = "VideoNow: one video production application (timeline, motion graphics, image editing)";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    # the workspace needs Rust 1.95 or newer (edition 2024, let chains)
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, rust-overlay }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" ];
      forAll = f: nixpkgs.lib.genAttrs systems (system:
        f (import nixpkgs { inherit system; overlays = [ rust-overlay.overlays.default ]; }));

      # loaded at run time by wgpu / winit (dlopen), so they go on LD_LIBRARY_PATH
      runtimeLibs = pkgs: with pkgs; [
        vulkan-loader
        libGL
        wayland
        libxkbcommon
        xorg.libX11
        xorg.libXcursor
        xorg.libXi
        xorg.libXrandr
        fontconfig
      ];

      toolchain = pkgs: pkgs.rust-bin.stable.latest.minimal;
    in
    {
      packages = forAll (pkgs:
        let
          rust = toolchain pkgs;
          rustPlatform = pkgs.makeRustPlatform { cargo = rust; rustc = rust; };
          libs = pkgs.lib.makeLibraryPath (runtimeLibs pkgs);
        in
        rec {
          videonow = rustPlatform.buildRustPackage {
            pname = "videonow";
            version = "0.1.0";
            src = pkgs.lib.cleanSourceWith {
              src = ./.;
              filter = path: type: !(pkgs.lib.hasInfix "/target" path);
            };
            cargoLock = {
              lockFile = ./Cargo.lock;
              # craft-libs and winprint.rs come from git
              allowBuiltinFetchGit = true;
            };
            cargoBuildFlags = [ "-p" "videonow-cli" "-p" "videonow-desktop" ];
            # the engines' test suites take hours; `nix flake check` runs the studio's own
            doCheck = false;
            nativeBuildInputs = [ pkgs.makeWrapper pkgs.pkg-config ];
            buildInputs = runtimeLibs pkgs;
            postInstall = ''
              wrapProgram $out/bin/videonow-desktop --prefix LD_LIBRARY_PATH : ${libs}
              install -Dm644 assets/brand/videonow-mark.png $out/share/icons/hicolor/512x512/apps/videonow.png
              install -Dm644 packaging/videonow.desktop $out/share/applications/videonow.desktop
            '';
            meta = {
              description = "One video production application: timeline, motion graphics and image editing in one project";
              homepage = "https://github.com/GutMutCode/videonow";
              license = with pkgs.lib.licenses; [ mit asl20 ];
              mainProgram = "videonow-desktop";
              platforms = systems;
            };
          };
          default = videonow;
        });

      apps = forAll (pkgs: {
        default = { type = "app"; program = "${self.packages.${pkgs.stdenv.hostPlatform.system}.videonow}/bin/videonow-desktop"; };
        cli = { type = "app"; program = "${self.packages.${pkgs.stdenv.hostPlatform.system}.videonow}/bin/videonow"; };
      });

      checks = forAll (pkgs:
        let
          rust = toolchain pkgs;
          rustPlatform = pkgs.makeRustPlatform { cargo = rust; rustc = rust; };
        in
        {
          studio = rustPlatform.buildRustPackage {
            pname = "videonow-studio-tests";
            version = "0.1.0";
            src = pkgs.lib.cleanSourceWith {
              src = ./.;
              filter = path: type: !(pkgs.lib.hasInfix "/target" path);
            };
            cargoLock = { lockFile = ./Cargo.lock; allowBuiltinFetchGit = true; };
            cargoBuildFlags = [ "-p" "videonow-cli" ];
            cargoTestFlags = [ "-p" "videonow-core" "-p" "videonow-eval" ];
            installPhase = "mkdir -p $out";
          };
        });

      devShells = forAll (pkgs: {
        default = pkgs.mkShell {
          packages = [ (pkgs.rust-bin.stable.latest.default.override { extensions = [ "rust-src" "clippy" "rustfmt" ]; }) pkgs.pkg-config ];
          buildInputs = runtimeLibs pkgs;
          LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath (runtimeLibs pkgs);
        };
      });
    };
}
