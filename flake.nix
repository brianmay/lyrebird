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
      in
      {
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
