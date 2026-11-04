# Root CMake entry point for the native part of the platform.
#
# The Rust workspace is built by cargo (see Cargo.toml); CMake builds only the
# Qt/C++ shell in apps/dj-ui. M0 scope: a blank QML window that starts and exits
# cleanly under -platform offscreen (scripts/ui.sh --smoke).
cmake_minimum_required(VERSION 3.24)

project(dj-platform LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_EXTENSIONS OFF)

if(NOT CMAKE_BUILD_TYPE AND NOT CMAKE_CONFIGURATION_TYPES)
  set(CMAKE_BUILD_TYPE Debug CACHE STRING "" FORCE)
endif()

add_subdirectory(apps/dj-ui)
