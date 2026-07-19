{ pkgs ? import <nixpkgs> { } }:

with pkgs;

mkShell {
  name = "limux-dev";

  nativeBuildInputs = [
    cargo
    rustc
    pkg-config
    wrapGAppsHook4
    zig_0_15       # to build libghostty.so
  ];

  buildInputs = [
    # Rust GTK/WebKit crate dependencies (found via pkg-config)
    gtk4 libadwaita webkitgtk_6_0 libsoup_3
    cairo pango harfbuzz gdk-pixbuf graphene
    glib libepoxy vulkan-loader wayland

    # libghostty GL deps
    libGL
    libxkbcommon fontconfig
  ];

  # pkg-config needs .dev outputs — use lib.getDev for those that have it,
  # bare package for single-output ones
  PKG_CONFIG_PATH = lib.makeSearchPath "lib/pkgconfig" [
    (lib.getDev gtk4) (lib.getDev libadwaita) (lib.getDev webkitgtk_6_0) (lib.getDev libsoup_3)
    (lib.getDev cairo) (lib.getDev pango) (lib.getDev harfbuzz) (lib.getDev gdk-pixbuf)
    (lib.getDev graphene) (lib.getDev glib) (lib.getDev libepoxy) (lib.getDev vulkan-loader)
    (lib.getDev wayland) (lib.getDev libxkbcommon) (lib.getDev fontconfig) libGL
  ];

  LD_LIBRARY_PATH = lib.makeLibraryPath [
    gtk4 libadwaita webkitgtk_6_0 libsoup_3 cairo pango harfbuzz gdk-pixbuf
    graphene glib libepoxy vulkan-loader wayland libxkbcommon fontconfig libGL
  ];

  WEBKIT_EXEC_PATH = "${webkitgtk_6_0}/libexec/webkitgtk-6.0";

  shellHook = ''
    echo "limux dev shell — nix-shell"
    echo "  cargo build  ← run this"
  '';
}
