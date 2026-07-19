{
  description = "Limux — GPU-accelerated terminal workspace manager";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  };

  outputs = { self, nixpkgs }:
    let
      forAllSystems = nixpkgs.lib.genAttrs nixpkgs.lib.systems.flakeExposed;
    in {
      devShells = forAllSystems (system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
        in {
          default = pkgs.mkShell {
            name = "limux-dev";
            nativeBuildInputs = with pkgs; [
              cargo rustc pkg-config zig_0_15 ncurses rustPlatform.bindgenHook
            ];
            buildInputs = with pkgs; [
              gtk4 libadwaita libepoxy webkitgtk_6_0 gtk4-layer-shell
              fontconfig freetype harfbuzz
            ];
            PKG_CONFIG_PATH = "${pkgs.gtk4-layer-shell.dev}/lib/pkgconfig";
            shellHook = ''
              export RUSTFLAGS="-C link-arg=-Wl,-rpath,${pkgs.libepoxy}/lib $RUSTFLAGS"
              addToSearchPath LD_LIBRARY_PATH "$PWD/ghostty/zig-out/lib"
              echo "Limux dev shell. Steps: git submodule update --init --recursive"
              echo "  cd ghostty && zig build -Dapp-runtime=none -Doptimize=ReleaseFast -Dcpu=baseline && cd .."
              echo "  cargo build --release"
              echo "  LD_LIBRARY_PATH=${pkgs.libepoxy}/lib ./target/release/limux"
            '';
          };
        });

      packages = forAllSystems (system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
        in {
          limux = pkgs.rustPlatform.buildRustPackage {
            pname = "limux";
            version = "0.1.21";
            src = ./.;
            cargoLock.lockFile = ./Cargo.lock;

            nativeBuildInputs = with pkgs; [
              pkg-config rustPlatform.bindgenHook zig_0_15 ncurses makeWrapper
            ];
            buildInputs = with pkgs; [
              gtk4 libadwaita libepoxy webkitgtk_6_0 gtk4-layer-shell
              fontconfig freetype harfbuzz
            ];

            PKG_CONFIG_PATH = "${pkgs.gtk4-layer-shell.dev}/lib/pkgconfig";

            preBuild = ''
              cat > .cargo/config.toml <<'CARGOEOF'
              [target.x86_64-unknown-linux-gnu]
              rustflags = ["-C", "link-arg=-Wl,-rpath,$out/lib"]
              [target.aarch64-unknown-linux-gnu]
              rustflags = ["-C", "link-arg=-Wl,-rpath,$out/lib"]
              CARGOEOF
              pushd ghostty
                zig build -Dapp-runtime=none -Doptimize=ReleaseFast -Dcpu=baseline
              popd
            '';

            postInstall = ''
              mkdir -p $out/lib $out/share/limux/ghostty $out/share/terminfo
              cp ghostty/zig-out/lib/libghostty.so $out/lib/
              if [ -d ghostty/zig-out/share/ghostty ]; then
                cp -r ghostty/zig-out/share/ghostty/. $out/share/limux/ghostty/
              fi
              for tdir in ghostty/zig-out/share/terminfo; do
                if [ -d "$tdir" ]; then
                  for f in "$tdir"/g/ghostty "$tdir"/x/xterm-ghostty; do
                    [ -f "$f" ] || continue
                    d=$(dirname "$f"); b=$(basename "$f")
                    mkdir -p "$out/share/terminfo/$(basename "$d")"
                    cp "$f" "$out/share/terminfo/$(basename "$d")/$b"
                  done
                  break
                fi
              done
              wrapProgram $out/bin/limux \
                --prefix LD_LIBRARY_PATH : $out/lib \
                --prefix XDG_DATA_DIRS : $out/share
            '';
          };
          default = self.packages.${system}.limux;
        });
    };
}
