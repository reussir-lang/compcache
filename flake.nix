{
  description = "Synchronous compiler cache: Rust, SQLite, Nickel and P";
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    fenix = {
      url = "github:nix-community/fenix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    flake-utils.url = "github:numtide/flake-utils";
  };
  outputs = { nixpkgs, fenix, flake-utils, ... }:
    flake-utils.lib.eachSystem [ "x86_64-linux" "aarch64-linux" ] (system:
      let
        pkgs = nixpkgs.legacyPackages.${system};
        rust = fenix.packages.${system}.stable.withComponents [
          "cargo" "rustc" "rustfmt" "clippy"
        ];
      in {
        devShells.default = pkgs.mkShell {
          packages = [ rust pkgs.sqlite pkgs.nickel pkgs.typst ];
        };
        # The checker has a separate shell: ordinary Rust builds need no .NET.
        devShells.proof = pkgs.mkShell {
          packages = [ pkgs.dotnet-sdk_8 ];
        };
      });
}
