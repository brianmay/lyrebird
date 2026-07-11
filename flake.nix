{
  description = "lyrebird — identify and rename HandBrake rips using TMDB metadata";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = nixpkgs.legacyPackages.${system};

        lyrebird = pkgs.rustPlatform.buildRustPackage {
          pname = "lyrebird";
          version = "0.1.0";
          # Only the crate itself: edits to flake.nix, CLAUDE.md etc. don't
          # invalidate the build.
          src = pkgs.lib.fileset.toSource {
            root = ./.;
            fileset = pkgs.lib.fileset.unions [
              ./Cargo.toml
              ./Cargo.lock
              ./src
            ];
          };
          cargoLock.lockFile = ./Cargo.lock;

          nativeBuildInputs = with pkgs; [ pkg-config makeWrapper ];
          buildInputs = with pkgs; [ openssl ];

          # The binary shells out to ffmpeg/ffprobe at runtime; wrap so the
          # installed package works without ffmpeg in the user's profile.
          postInstall = ''
            wrapProgram $out/bin/lyrebird \
              --prefix PATH : ${pkgs.lib.makeBinPath [ pkgs.ffmpeg ]}
          '';

          meta = {
            description = "Identify and rename HandBrake video rips using TMDB metadata";
            mainProgram = "lyrebird";
          };
        };
      in
      {
        packages.default = lyrebird;

        # `nix flake check` builds everything under checks — this makes it
        # build the package, whose checkPhase runs the cargo test suite.
        checks.build = lyrebird;

        devShells.default = pkgs.mkShell {
          packages = with pkgs; [
            rustc
            cargo
            rust-analyzer
            clippy
            rustfmt

            # reqwest's default native-tls backend links against openssl
            pkg-config
            openssl

            # ffprobe (duration cross-check) and ffmpeg (contact sheets)
            ffmpeg
          ];

          env.RUST_SRC_PATH = "${pkgs.rustPlatform.rustLibSrc}";
        };
      });
}
