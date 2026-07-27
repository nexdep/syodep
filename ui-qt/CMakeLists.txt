find_package(Qt6 REQUIRED COMPONENTS Widgets OpenGL OpenGLWidgets)

# Manual equivalent of qt_standard_project_setup(), which needs Qt >= 6.3;
# the AppImage builds against Ubuntu 22.04's Qt 6.2.
set(CMAKE_AUTOMOC ON)
set(CMAKE_AUTOUIC ON)
set(CMAKE_AUTORCC ON)

qt_add_executable(syodep
    src/main.cpp
    src/main_window.cpp
    src/main_window.h
    src/canvas_widget.cpp
    src/canvas_widget.h
    src/diagnostics.cpp
    src/diagnostics.h
    src/key_encoder.cpp
    src/key_encoder.h)

target_link_libraries(syodep PRIVATE
    Qt6::Widgets
    Qt6::OpenGL
    Qt6::OpenGLWidgets
    syodep_ffi)

# Surfaced by `--version`/`--check`. Falls back to "unknown" when unset.
# SYODEP_VERSION is read straight out of Cargo.toml by the top-level
# CMakeLists.txt (full string, so pre-release suffixes survive), so the shell
# cannot report a different version from the core it links against.
target_compile_definitions(syodep PRIVATE
    SYODEP_BUILD_TYPE="$<IF:$<BOOL:$<CONFIG>>,$<CONFIG>,unknown>"
    SYODEP_VERSION="${SYODEP_VERSION}")

set_target_properties(syodep PROPERTIES
    WIN32_EXECUTABLE TRUE)

# --- Windows icon + version resource -----------------------------------------
# The .ico is generated from packaging/syodep.svg rather than committed, since
# AGENTS.md forbids checked-in generated binaries. ImageMagick is preinstalled
# on the CI runners; when it is missing the resource is skipped entirely so a
# developer without it still gets a working (if generic-looking) build.
if(WIN32)
    find_program(MAGICK_EXECUTABLE NAMES magick convert)
    if(MAGICK_EXECUTABLE)
        set(SYODEP_ICON "${CMAKE_CURRENT_BINARY_DIR}/syodep.ico")
        set(SYODEP_ICON_SVG "${CMAKE_SOURCE_DIR}/packaging/syodep.svg")
        add_custom_command(
            OUTPUT "${SYODEP_ICON}"
            COMMAND "${MAGICK_EXECUTABLE}" -background none "${SYODEP_ICON_SVG}"
                    -define icon:auto-resize=256,128,64,48,32,24,16
                    "${SYODEP_ICON}"
            DEPENDS "${SYODEP_ICON_SVG}"
            COMMENT "Generating syodep.ico from packaging/syodep.svg"
            VERBATIM)

        # rc.exe accepts forward slashes; backslashes would need escaping.
        set(SYODEP_ICON_RC_PATH "${SYODEP_ICON}")
        # VERSIONINFO wants four comma-separated numbers.
        string(REPLACE "." "," SYODEP_VERSION_COMMA "${SYODEP_VERSION_NUMERIC}")
        set(SYODEP_VERSION_COMMA "${SYODEP_VERSION_COMMA},0")
        configure_file(syodep.rc.in "${CMAKE_CURRENT_BINARY_DIR}/syodep.rc" @ONLY)

        target_sources(syodep PRIVATE
            "${CMAKE_CURRENT_BINARY_DIR}/syodep.rc"
            "${SYODEP_ICON}")
    else()
        message(WARNING
            "ImageMagick not found: syodep.exe will have no icon or version "
            "resource. Install it to build a properly branded binary.")
    endif()
endif()
