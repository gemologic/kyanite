{ pkgs ? import
    (fetchTarball {
      name = "jpetrucciani-2026-01-04";
      url = "https://github.com/jpetrucciani/nix/archive/c636141ffde1c268a5ddd6edea0a4679fc258f2b.tar.gz";
      sha256 = "1fwmax5rvb3nbwimknvbx4j4yyp2s5q1r10qkplhgpns8km8nck2";
    })
    { overlays = [ _rust ]; }
, _rust ? import
    (fetchTarball {
      name = "rust-overlay-2026-01-04";
      url = "https://github.com/oxalica/rust-overlay/archive/cb24c5cc207ba8e9a4ce245eedd2d37c3a988bc1.tar.gz";
      sha256 = "096lirg41f5vgq9rrfg5b6vzyrya8v472v6cqfh1hjfi9ys20hc4";
    })
}:
let
  name = "kyanite";

  rust = pkgs.rust-bin.selectLatestNightlyWith (toolchain: toolchain.default.override {
    extensions = [ "rust-analyzer" "rust-src" "rustc-dev" ];
    targets = [
      "x86_64-pc-windows-gnu"
      "x86_64-unknown-linux-musl"
    ];
  });

  mingw = pkgs.pkgsCross.mingwW64;

  tools = with pkgs; {
    cli = [
      jfmt
      nixup
    ];
    rust = [
      rust
      pkg-config
      mingw.stdenv.cc
      mingw.windows.pthreads
    ];
    scripts = pkgs.lib.attrsets.attrValues scripts;
  };

  scripts = { };
  paths = pkgs.lib.flatten [ (builtins.attrValues tools) ];
  env = pkgs.buildEnv {
    inherit name paths; buildInputs = paths;
  };
in
(env.overrideAttrs (_: {
  inherit name;
  NIXUP = "0.0.9";
})) // { inherit scripts; }
