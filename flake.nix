{
  description = "QzoneArchive local QQ space archive tool";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs =
    {
      self,
      nixpkgs,
      flake-utils,
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs { inherit system; };
        qzonearchive = import ./nix/qzonearchive.nix { inherit pkgs; };
      in
      {
        packages.default = qzonearchive;
        packages.qzonearchive = qzonearchive;
      }
    );
}
