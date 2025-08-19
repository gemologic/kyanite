{ pkgs ? import
    (fetchTarball {
      name = "jpetrucciani-2025-08-18";
      url = "https://github.com/jpetrucciani/nix/archive/bb541da0233d0d8bec1d738d9b41ff352b283038.tar.gz";
      sha256 = "1dc1z21xv0s6mh9ybgbvnvm7fcay729mgg0ypjwzkgqxyk2pnyx3";
    })
    { }
}:
let
  name = "kyanite";

  tools = with pkgs; {
    cli = [
      jfmt
      nixup
    ];
    rust = [
      cargo
      clang
      rust-analyzer
      rustc
      rustfmt
      # deps
      pkg-config
      openssl
    ];
    scripts = pkgs.lib.attrsets.attrValues scripts;
  };

  scripts = with pkgs; { };
  paths = pkgs.lib.flatten [ (builtins.attrValues tools) ];
  env = pkgs.buildEnv {
    inherit name paths; buildInputs = paths;
  };
in
(env.overrideAttrs (_: {
  inherit name;
  NIXUP = "0.0.9";
})) // { inherit scripts; }
