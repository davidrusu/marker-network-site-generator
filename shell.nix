{ pkgs ? import <nixpkgs> { } }:
with pkgs;
mkShell {
  buildInputs = [
    rustup
    openssl
    pkg-config
    sqlite
  ];
}
