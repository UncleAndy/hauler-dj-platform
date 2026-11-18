# Qt/CMake for Windows, built on a Linux host.
#
# The shell is a Qt Quick program; on Windows it is compiled with mingw-w64, the same compiler family the Qt
# binaries themselves were built with. CMake has to be told three things: the target system, the compiler
# prefix, and that it must not try to run what it builds.
set(CMAKE_SYSTEM_NAME Windows)
set(CMAKE_SYSTEM_PROCESSOR x86_64)

set(TOOLCHAIN_PREFIX x86_64-w64-mingw32)

set(CMAKE_C_COMPILER ${TOOLCHAIN_PREFIX}-gcc)
set(CMAKE_CXX_COMPILER ${TOOLCHAIN_PREFIX}-g++)
set(CMAKE_RC_COMPILER ${TOOLCHAIN_PREFIX}-windres)

set(CMAKE_FIND_ROOT_PATH /usr/${TOOLCHAIN_PREFIX})
set(CMAKE_FIND_ROOT_PATH_MODE_PROGRAM NEVER)
set(CMAKE_FIND_ROOT_PATH_MODE_LIBRARY ONLY)
set(CMAKE_FIND_ROOT_PATH_MODE_INCLUDE ONLY)

# Qt's own deployment tool and its libraries are executables of the target: nothing here is run on the host.
set(CMAKE_CROSSCOMPILING_EMULATOR "")
