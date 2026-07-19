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
            LD_LIBRARY_PATH = lib.makeLibraryPath (with pkgs; [
              gtk4 libadwaita libepoxy webkitgtk_6_0 libsoup_3
              cairo pango harfbuzz gdk-pixbuf graphene glib
              vulkan-loader wayland fontconfig freetype libxkbcommon libGL
            ]);
            shellHook = ''
              addToSearchPath LD_LIBRARY_PATH "$PWD/ghostty/zig-out/lib"
              echo "Limux dev shell. Steps:"
              echo '  cd ghostty && zig build -Dapp-runtime=none -Doptimize=ReleaseFast -Dcpu=baseline'
              echo '  cargo build'
              echo '  ./target/debug/limux'
            '';
          };
        });
    };
}
