{ inputs, ... }:
{
  perSystem =
    {
      self',
      lib,
      pkgs,
      ...
    }:
    let
      uwulib = inputs.self.uwulib.init pkgs;
      rocksdbAllFeatures = self'.packages.rocksdb.override {
        enableJemalloc = true;
      };
    in
    {
      # basic nix shell containing all things necessary to build continuwuity in all flavors manually (on x86_64-linux)
      devShells.default = uwulib.build.craneLib.devShell {
        packages = [
          pkgs.nodejs
          pkgs.pkg-config
          pkgs.liburing
          pkgs.rust-jemalloc-sys-unprefixed
          rocksdbAllFeatures
          # Pre commit support
          pkgs.pre-commit
          pkgs.typos
          pkgs.committed
          self'.packages.dev-toolchain
        ];
        env.LIBCLANG_PATH = lib.makeLibraryPath [ pkgs.llvmPackages.libclang.lib ];

        env.LD_LIBRARY_PATH =
          with pkgs;
          lib.makeLibraryPath [
            pkg-config
            liburing
            rust-jemalloc-sys-unprefixed
            rocksdbAllFeatures
            llvmPackages.libclang.lib
            stdenv.cc.cc.lib
          ];

        shellHook = ''
          pre-commit install 2>/dev/null || true
        '';
      };
    };
}
