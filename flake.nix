{
  description = "DJ Platform — independent DJ software for Linux/NixOS (Rust core + Qt 6/QML UI)";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-26.05";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    # Linux only (DJP-ENV-001): the shell contains ALSA/PipeWire/Qt, which do not exist on
    # Darwin. Windows/macOS become separate adapters after ADR-006.
    flake-utils.lib.eachSystem [ "x86_64-linux" ] (system:
      let
        pkgs = import nixpkgs { inherit system; };

        # Qt modules the UI shell links against. qtwayland is only needed so the
        # shell can actually start on the owner's Wayland session; the build itself
        # needs qtbase + qtdeclarative.
        qt = pkgs.qt6;
        qtModules = with qt; [ qtbase qtdeclarative ];
        qtRuntime = with qt; [ qtbase qtdeclarative qtwayland ];

        # Native libraries whose build scripts probe the environment:
        # cpal -> alsa-sys (bindgen) and libspa-sys (PipeWire headers), openssl for tls.
        pkgConfigPath = pkgs.lib.makeSearchPath "lib/pkgconfig" [
          pkgs.alsa-lib.dev
          pkgs.pipewire.dev
          pkgs.openssl.dev
        ];

        # Libraries the binaries dlopen or link at run time: GL / Wayland / xkb /
        # fontconfig for the scene graph, ALSA + PipeWire for the audio backend.
        runtimeLibs = with pkgs; [
          alsa-lib
          pipewire
          openssl
          libGL
          libglvnd
          wayland
          libxkbcommon
          fontconfig
          freetype
          zlib
          stdenv.cc.cc.lib
        ] ++ qtRuntime;

        # Written into .cargo/config.toml by the shell hook so cargo subprocesses
        # (and rust-analyzer outside the shell) see the same bindgen/pkg-config setup.
        cargoConfig = pkgs.writeText "dj-platform-cargo-config.toml" ''
          [env]
          PKG_CONFIG_PATH = { value = "${pkgConfigPath}", relative = false }
          LIBCLANG_PATH = { value = "${pkgs.llvmPackages.libclang.lib}/lib", relative = false }
          BINDGEN_EXTRA_CLANG_ARGS = { value = "-isystem ${pkgs.glibc.dev}/include", relative = false }
        '';
      in
      {
        formatter = pkgs.nixfmt-rfc-style;

        devShells.default = pkgs.mkShell {
          name = "dj-platform-dev";

          nativeBuildInputs = with pkgs; [
            pkg-config
            clang
            cmake
            ninja
            gnumake
            rustc
            cargo
            rustfmt
            clippy
            rust-analyzer
            llvmPackages.libclang
            qt.qtdeclarative # qmllint, qmlformat, qmltestrunner
          ];

          buildInputs = with pkgs; [
            # NOTE: do NOT add glibc.dev to buildInputs here. It puts -isystem <glibc-dev>/include
            # ahead of libstdc++ in the include search order, and then <cstdlib> -> #include_next
            # <stdlib.h> fails ("stdlib.h: No such file"). bindgen gets the headers via
            # BINDGEN_EXTRA_CLANG_ARGS below, which is all the Rust side needs.
            alsa-lib
            pipewire
            openssl
            libGL
            libglvnd
            wayland
            libxkbcommon
          ] ++ qtModules;

          # Keep cargo artifacts out of the checkout: the tree is shared between the
          # owner and the agent, and a target/ directory owned by one of them breaks
          # the other. Override CARGO_TARGET_DIR to change it.
          shellHook = ''
            export CARGO_TARGET_DIR="''${CARGO_TARGET_DIR:-''${XDG_CACHE_HOME:-$HOME/.cache}/dj-platform-target}"

            export PKG_CONFIG_PATH="${pkgConfigPath}:''${PKG_CONFIG_PATH:-}"
            export LIBCLANG_PATH="${pkgs.llvmPackages.libclang.lib}/lib"
            export BINDGEN_EXTRA_CLANG_ARGS="-isystem ${pkgs.glibc.dev}/include"

            export CMAKE_PREFIX_PATH="${qt.qtbase}:${qt.qtdeclarative}:''${CMAKE_PREFIX_PATH:-}"
            export QT_PLUGIN_PATH="${qt.qtbase}/lib/qt-6/plugins:${qt.qtwayland}/lib/qt-6/plugins"
            export QML2_IMPORT_PATH="${qt.qtdeclarative}/lib/qt-6/qml:${qt.qtbase}/lib/qt-6/qml"
            export LD_LIBRARY_PATH="${pkgs.lib.makeLibraryPath runtimeLibs}:''${LD_LIBRARY_PATH:-}"

            if ! cmp -s ${cargoConfig} .cargo/config.toml 2>/dev/null; then
              mkdir -p .cargo
              cp ${cargoConfig} .cargo/config.toml 2>/dev/null \
                || echo "dj-platform: .cargo/config.toml is not writable; env vars are set for this shell only" >&2
            fi

            export DJ_PLATFORM_DEVSHELL=1
            echo "dj-platform dev shell — rust $(rustc --version | cut -d" " -f2), qt ${qt.qtbase.version}, cmake $(cmake --version | head -1 | cut -d" " -f3)"
            echo "  cargo test --workspace    bash scripts/check.sh    bash scripts/ui.sh --smoke"
          '';
        };
      });
}
