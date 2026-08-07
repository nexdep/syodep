# Top-level build: compiles the Rust core (as a static library with a C ABI)
# via cargo, then the Qt 6 shell against it.
#
#   cmake -B build -G Ninja
#   cmake --build build
#   ./build/ui-qt/syodep document.pdf
#
# The Rust workspace also builds/tests standalone with plain `cargo test`;
# CMake is only required for the desktop shell.

cmake_minimum_required(VERSION 3.21)
# Cargo.toml holds the only manually maintained base version. Read it rather
# than keeping a second copy; the channel-aware identity derived below is
# injected into both the Qt shell and Rust core.
file(READ "${CMAKE_CURRENT_SOURCE_DIR}/Cargo.toml" _syodep_cargo_toml)
# `[^[]*` stops the match at the next section header, so dependency versions in
# [workspace.dependencies] can never be picked up by mistake.
string(REGEX MATCH "\\[workspace\\.package\\][^[]*version[ \t]*=[ \t]*\"([^\"]+)\""
    _syodep_version_match "${_syodep_cargo_toml}")
set(SYODEP_VERSION "${CMAKE_MATCH_1}")
if(NOT SYODEP_VERSION)
    message(FATAL_ERROR
        "could not read workspace.package.version from Cargo.toml")
endif()
# project() accepts digits and dots only, so a pre-release like 0.5.0-rc1 keeps
# its suffix for display while project() gets the numeric part.
string(REGEX MATCH "^[0-9]+(\\.[0-9]+)*" SYODEP_VERSION_NUMERIC "${SYODEP_VERSION}")
project(syodep VERSION ${SYODEP_VERSION_NUMERIC} LANGUAGES CXX)
# Windows resources (icon + VERSIONINFO). Enabled here rather than in
# project(LANGUAGES) so non-Windows builds never look for a resource compiler.
if(WIN32)
    enable_language(RC)
endif()
# Re-run configure when the version changes, so a bump is picked up without a
# manual cmake invocation.
set_property(DIRECTORY APPEND PROPERTY CMAKE_CONFIGURE_DEPENDS
    "${CMAKE_CURRENT_SOURCE_DIR}/Cargo.toml")

# Cargo.toml remains the only manually maintained version. Distribution builds
# add a channel and commit to that base so a rolling binary cannot masquerade
# as the regular release with the same Cargo version.
set(SYODEP_BUILD_CHANNEL "auto" CACHE STRING
    "User-visible build channel: auto, release, continuous, or development")
set_property(CACHE SYODEP_BUILD_CHANNEL PROPERTY STRINGS
    auto release continuous development)
string(TOLOWER "${SYODEP_BUILD_CHANNEL}" _syodep_requested_channel)
if(NOT _syodep_requested_channel MATCHES
        "^(auto|release|continuous|development)$")
    message(FATAL_ERROR
        "unknown SYODEP_BUILD_CHANNEL='${SYODEP_BUILD_CHANNEL}'")
endif()

find_package(Git QUIET)
set(_syodep_commit "unknown")
set(_syodep_git_available false)
set(_syodep_git_clean false)
set(_syodep_exact_release_tag false)
if(Git_FOUND)
    execute_process(
        COMMAND "${GIT_EXECUTABLE}" -C "${CMAKE_CURRENT_SOURCE_DIR}"
                rev-parse --short=12 HEAD
        OUTPUT_VARIABLE _syodep_git_commit
        OUTPUT_STRIP_TRAILING_WHITESPACE
        RESULT_VARIABLE _syodep_git_commit_result)
    if(_syodep_git_commit_result EQUAL 0 AND _syodep_git_commit)
        set(_syodep_commit "${_syodep_git_commit}")
        set(_syodep_git_available true)
    endif()

    execute_process(
        COMMAND "${GIT_EXECUTABLE}" -C "${CMAKE_CURRENT_SOURCE_DIR}"
                status --porcelain --untracked-files=normal
        OUTPUT_VARIABLE _syodep_git_status
        OUTPUT_STRIP_TRAILING_WHITESPACE
        RESULT_VARIABLE _syodep_git_status_result)
    if(_syodep_git_status_result EQUAL 0 AND NOT _syodep_git_status)
        set(_syodep_git_clean true)
    endif()

    execute_process(
        COMMAND "${GIT_EXECUTABLE}" -C "${CMAKE_CURRENT_SOURCE_DIR}"
                describe --tags --exact-match --match "v${SYODEP_VERSION}"
        OUTPUT_QUIET
        ERROR_QUIET
        RESULT_VARIABLE _syodep_release_tag_result)
    if(_syodep_release_tag_result EQUAL 0)
        set(_syodep_exact_release_tag true)
    endif()

    # Reconfigure after a commit/checkout so local development identities do
    # not stay pinned to the commit that first populated the build directory.
    execute_process(
        COMMAND "${GIT_EXECUTABLE}" -C "${CMAKE_CURRENT_SOURCE_DIR}"
                rev-parse --git-path HEAD
        OUTPUT_VARIABLE _syodep_git_head
        OUTPUT_STRIP_TRAILING_WHITESPACE)
    execute_process(
        COMMAND "${GIT_EXECUTABLE}" -C "${CMAKE_CURRENT_SOURCE_DIR}"
                rev-parse --git-path index
        OUTPUT_VARIABLE _syodep_git_index
        OUTPUT_STRIP_TRAILING_WHITESPACE)
    execute_process(
        COMMAND "${GIT_EXECUTABLE}" -C "${CMAKE_CURRENT_SOURCE_DIR}"
                symbolic-ref -q HEAD
        OUTPUT_VARIABLE _syodep_git_ref
        OUTPUT_STRIP_TRAILING_WHITESPACE)
    foreach(_syodep_git_path IN ITEMS
            "${_syodep_git_head}" "${_syodep_git_index}")
        if(_syodep_git_path AND NOT IS_ABSOLUTE "${_syodep_git_path}")
            set(_syodep_git_path
                "${CMAKE_CURRENT_SOURCE_DIR}/${_syodep_git_path}")
        endif()
        if(EXISTS "${_syodep_git_path}")
            set_property(DIRECTORY APPEND PROPERTY CMAKE_CONFIGURE_DEPENDS
                "${_syodep_git_path}")
        endif()
    endforeach()
    if(_syodep_git_ref)
        execute_process(
            COMMAND "${GIT_EXECUTABLE}" -C "${CMAKE_CURRENT_SOURCE_DIR}"
                    rev-parse --git-path "${_syodep_git_ref}"
            OUTPUT_VARIABLE _syodep_git_ref_path
            OUTPUT_STRIP_TRAILING_WHITESPACE)
        if(NOT IS_ABSOLUTE "${_syodep_git_ref_path}")
            set(_syodep_git_ref_path
                "${CMAKE_CURRENT_SOURCE_DIR}/${_syodep_git_ref_path}")
        endif()
        if(EXISTS "${_syodep_git_ref_path}")
            set_property(DIRECTORY APPEND PROPERTY CMAKE_CONFIGURE_DEPENDS
                "${_syodep_git_ref_path}")
        endif()
    endif()
endif()
string(LENGTH "$ENV{GITHUB_SHA}" _syodep_github_sha_length)
if(_syodep_commit STREQUAL "unknown"
        AND "$ENV{GITHUB_SHA}" MATCHES "^[0-9A-Fa-f]+$"
        AND _syodep_github_sha_length GREATER_EQUAL 12)
    string(SUBSTRING "$ENV{GITHUB_SHA}" 0 12 _syodep_commit)
endif()

if(_syodep_requested_channel STREQUAL "auto")
    if(_syodep_exact_release_tag AND _syodep_git_clean)
        set(_syodep_resolved_channel "release")
    else()
        set(_syodep_resolved_channel "development")
    endif()
else()
    set(_syodep_resolved_channel "${_syodep_requested_channel}")
endif()
if(_syodep_resolved_channel STREQUAL "release" AND _syodep_git_available
        AND (NOT _syodep_exact_release_tag OR NOT _syodep_git_clean))
    message(FATAL_ERROR
        "release builds require a clean checkout at tag v${SYODEP_VERSION}")
endif()

include("${CMAKE_CURRENT_SOURCE_DIR}/cmake/SyodepBuildIdentity.cmake")
syodep_format_build_version(
    "${SYODEP_VERSION}" "${_syodep_resolved_channel}"
    "${_syodep_commit}" SYODEP_BUILD_VERSION)
message(STATUS
    "syodep build identity: ${SYODEP_BUILD_VERSION} (${_syodep_resolved_channel})")
file(WRITE "${CMAKE_BINARY_DIR}/syodep-version.txt"
    "${SYODEP_BUILD_VERSION}\n")

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

# --- Rust core (syodep-ffi staticlib + generated header) --------------------

find_program(CARGO_EXECUTABLE cargo REQUIRED
    HINTS "$ENV{HOME}/.cargo/bin" "$ENV{USERPROFILE}/.cargo/bin")

# Release Rust by default even in Debug C++ builds: debug MuPDF rendering is
# too slow to be usable. Override with -DSYODEP_RUST_PROFILE=dev if needed.
set(SYODEP_RUST_PROFILE "release" CACHE STRING "cargo profile for the core")
if(SYODEP_RUST_PROFILE STREQUAL "dev")
    set(RUST_TARGET_DIR "${CMAKE_CURRENT_SOURCE_DIR}/target/debug")
    set(CARGO_PROFILE_FLAG "")
else()
    set(RUST_TARGET_DIR "${CMAKE_CURRENT_SOURCE_DIR}/target/${SYODEP_RUST_PROFILE}")
    set(CARGO_PROFILE_FLAG "--profile=${SYODEP_RUST_PROFILE}")
endif()

if(WIN32)
    set(SYODEP_FFI_LIB "${RUST_TARGET_DIR}/syodep_ffi.lib")
else()
    set(SYODEP_FFI_LIB "${RUST_TARGET_DIR}/libsyodep_ffi.a")
endif()
set(SYODEP_FFI_INCLUDE_DIR "${CMAKE_CURRENT_SOURCE_DIR}/crates/syodep-ffi/include")
# The header is generated by cargo (cbindgen) and gitignored; the directory
# must exist at configure time or CMake rejects the imported target's
# INTERFACE_INCLUDE_DIRECTORIES on a fresh checkout.
file(MAKE_DIRECTORY "${SYODEP_FFI_INCLUDE_DIR}")

file(GLOB_RECURSE SYODEP_RUST_SOURCES CONFIGURE_DEPENDS
    "${CMAKE_CURRENT_SOURCE_DIR}/crates/*.rs"
    "${CMAKE_CURRENT_SOURCE_DIR}/crates/*/Cargo.toml"
    "${CMAKE_CURRENT_SOURCE_DIR}/Cargo.toml")

add_custom_command(
    OUTPUT "${SYODEP_FFI_LIB}"
    COMMAND "${CMAKE_COMMAND}" -E env
            "SYODEP_BUILD_VERSION=${SYODEP_BUILD_VERSION}"
            "${CARGO_EXECUTABLE}" build -p syodep-ffi ${CARGO_PROFILE_FLAG}
    WORKING_DIRECTORY "${CMAKE_CURRENT_SOURCE_DIR}"
    DEPENDS ${SYODEP_RUST_SOURCES}
    COMMENT "Building Rust core (cargo build -p syodep-ffi, ${SYODEP_RUST_PROFILE})"
    VERBATIM
    USES_TERMINAL)
add_custom_target(syodep_ffi_build DEPENDS "${SYODEP_FFI_LIB}")

add_library(syodep_ffi STATIC IMPORTED GLOBAL)
add_dependencies(syodep_ffi syodep_ffi_build)
set_target_properties(syodep_ffi PROPERTIES
    IMPORTED_LOCATION "${SYODEP_FFI_LIB}"
    INTERFACE_INCLUDE_DIRECTORIES "${SYODEP_FFI_INCLUDE_DIR}")

# System libraries required by the Rust staticlib (and bundled MuPDF/SQLite).
# rustc knows the exact set: `--print=native-static-libs` (builds the crate
# as a side effect, which the custom command would do anyway).
if(WIN32)
    execute_process(
        COMMAND "${CMAKE_COMMAND}" -E env
                "SYODEP_BUILD_VERSION=${SYODEP_BUILD_VERSION}"
                "${CARGO_EXECUTABLE}" rustc --color=never -p syodep-ffi
                ${CARGO_PROFILE_FLAG} -- --print=native-static-libs
        WORKING_DIRECTORY "${CMAKE_CURRENT_SOURCE_DIR}"
        OUTPUT_VARIABLE _cargo_stdout
        ERROR_VARIABLE _cargo_stderr
        RESULT_VARIABLE _cargo_result)
    # Strip ANSI color codes (CARGO_TERM_COLOR=always in CI poisons tokens).
    string(ASCII 27 _esc)
    string(REGEX REPLACE "${_esc}\\[[0-9;]*m" "" _cargo_stderr "${_cargo_stderr}")
    string(REGEX MATCH "native-static-libs: ([^\r\n]*)" _match "${_cargo_stderr}")
    if(_cargo_result EQUAL 0 AND _match)
        separate_arguments(_raw_libs WINDOWS_COMMAND "${CMAKE_MATCH_1}")
        # rustc emits linker-flag tokens like `/defaultlib:msvcrt`; CMake
        # would treat them as file paths, so reduce them to library names.
        # MuPDF's static libs (libmupdf/libthirdparty) are not bundled into
        # syodep_ffi.lib; they live in mupdf-sys's cargo OUT_DIR, so resolve
        # them to full paths the consumer link can actually find.
        set(SYODEP_NATIVE_LIBS "")
        foreach(_lib IN LISTS _raw_libs)
            string(REGEX REPLACE "^[/-][Dd][Ee][Ff][Aa][Uu][Ll][Tt][Ll][Ii][Bb]:" "" _lib "${_lib}")
            if(_lib MATCHES "^lib(mupdf|thirdparty)")
                file(GLOB_RECURSE _found
                    "${RUST_TARGET_DIR}/build/mupdf-sys-*/out/${_lib}")
                if(_found)
                    list(GET _found 0 _lib)
                else()
                    message(WARNING "could not locate ${_lib} in cargo OUT_DIR")
                endif()
            endif()
            list(APPEND SYODEP_NATIVE_LIBS "${_lib}")
        endforeach()
        list(REMOVE_DUPLICATES SYODEP_NATIVE_LIBS)
        message(STATUS "Rust native static libs: ${SYODEP_NATIVE_LIBS}")
    else()
        # Fallback guess; kept in sync manually if the query ever fails.
        set(SYODEP_NATIVE_LIBS
            ws2_32 userenv bcrypt ntdll advapi32 secur32 ncrypt)
        message(WARNING "could not query native-static-libs from cargo "
            "(exit ${_cargo_result}); using fallback list")
    endif()
    set_property(TARGET syodep_ffi PROPERTY INTERFACE_LINK_LIBRARIES
        ${SYODEP_NATIVE_LIBS})
else()
    # fontconfig/freetype are required by the core's system-font discovery.
    set_property(TARGET syodep_ffi PROPERTY INTERFACE_LINK_LIBRARIES
        pthread dl m fontconfig freetype)
endif()

# --- Qt shell ----------------------------------------------------------------

add_subdirectory(ui-qt)
