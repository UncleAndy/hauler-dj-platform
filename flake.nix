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
        # One command for both halves of the application: the engine (cargo) and the shell (CMake + Qt). They are
        # built by different tools into different folders, and the launcher looks for them exactly there, so a
        # single command leaves the tree ready to start.
        #
        #   dj-build              engine and shell, release
        #   dj-build --debug      engine in debug (the shell has one profile)
        #   dj-build --engine-only / --ui-only
        #
        # Outside `nix develop` the script re-enters the shell itself: the pinned rust and Qt live there, and a
        # build made with the machine's own toolchain is the class of bug this project already knows.
        djBuild = pkgs.writeShellScriptBin "dj-build" ''
          set -eu

          profile=release
          want_engine=1
          want_ui=1

          usage() {
            cat <<'EOF'
          dj-build — build the engine and the shell.

            --release      engine in release (default)
            --debug        engine in debug; the shell has one profile
            --engine-only  only the engine
            --ui-only      only the shell
            -h, --help     this text

          Built where the launcher looks for them: the engine in $CARGO_TARGET_DIR/$profile, the shell in
          $XDG_CACHE_HOME/dj-platform-ui-build. Start it with: cargo run --release -p dj-launcher
          EOF
          }

          for arg in "$@"; do
            case "$arg" in
              --debug) profile=debug ;;
              --release) profile=release ;;
              --engine-only) want_ui=0 ;;
              --ui-only) want_engine=0 ;;
              -h|--help) usage; exit 0 ;;
              *) echo "dj-build: unknown option $arg (try --help)" >&2; exit 2 ;;
            esac
          done

          # Outside the dev shell there is no cargo and no Qt: enter it and come back here.
          if [ -z "''${DJ_PLATFORM_DEVSHELL:-}" ]; then
            if command -v nix >/dev/null 2>&1; then
              repo="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
              exec nix develop "$repo" --command dj-build "$@"
            fi
            echo "dj-build: run me inside 'nix develop' (cargo and Qt live there)" >&2
            exit 1
          fi

          repo="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
          cd "$repo"

          if [ "$want_engine" = 1 ]; then
            echo "== engine ($profile)"
            if [ "$profile" = debug ]; then
              cargo build -p dj-engine
            else
              cargo build --release -p dj-engine
            fi
            engine_dir="''${CARGO_TARGET_DIR:-$repo/target}/$profile"
            # ONNX Runtime is loaded from beside the engine, so a copy there is what makes the beat model work
            # without any environment variable at run time.
            if [ -n "''${DJ_ORT_LIBRARY:-}" ] && [ -f "$DJ_ORT_LIBRARY" ]; then
              cp -f "$DJ_ORT_LIBRARY" "$engine_dir/libonnxruntime.so"
            fi
            echo "   $engine_dir/dj-engine"
          fi

          if [ "$want_ui" = 1 ]; then
            echo "== shell (Qt 6, CMake)"
            ui_build="''${DJ_UI_BUILD_DIR:-''${XDG_CACHE_HOME:-$HOME/.cache}/dj-platform-ui-build}"
            BUILD_DIR="$ui_build" bash "$repo/scripts/ui.sh" --build-only
            echo "   $ui_build/apps/dj-ui/dj-ui"
          fi

          if [ "$profile" = debug ]; then
            echo "== ready: cargo run -p dj-launcher"
          else
            echo "== ready: cargo run --release -p dj-launcher"
          fi
        '';
      in
      {
        formatter = pkgs.nixfmt-rfc-style;

        # `nix run .#build` and `nix run` do the same thing from outside the shell.
        apps.build = {
          type = "app";
          program = "${djBuild}/bin/dj-build";
        };
        apps.default = {
          type = "app";
          program = "${djBuild}/bin/dj-build";
        };

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
            djBuild
          ];

          buildInputs = with pkgs; [
            # ONNX Runtime: движок грузит его динамически для внешней модели удара (ADR-0012).
            # Без библиотеки модель остаётся установленной, но не используется.
            onnxruntime
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

            # Путь к libonnxruntime.so: скрипты кладут копию рядом с dj-engine, и переменная
            # окружения при запуске уже не нужна.
            export DJ_ORT_LIBRARY="$(ls ${pkgs.onnxruntime}/lib/libonnxruntime.so* 2>/dev/null | head -1)"

            export DJ_PLATFORM_DEVSHELL=1
            echo "dj-platform dev shell — rust $(rustc --version | cut -d" " -f2), qt ${qt.qtbase.version}, cmake $(cmake --version | head -1 | cut -d" " -f3)"
            echo "  dj-build   (движок и оболочка одной командой)    cargo test --workspace    bash scripts/check.sh"
          '';
        };
      })
    // {
      # Real-time priority for the audio thread is granted by the system, not requested by the
      # application (ADR-0010): the app checks the policy it actually got and warns. The module
      # lives in its own file so it can be evaluated on its own.
      #
      #   imports = [ inputs.hauler-dj.nixosModules.dj-audio-rt ];
      #   services.dj-audio-rt.enable = true;
      #
      # The user still has to be in the `audio` group:
      #   users.users.<name>.extraGroups = [ "audio" ];
      nixosModules.dj-audio-rt = import ./nix/module.nix;
      nixosModules.default = self.nixosModules.dj-audio-rt;
    };
}
