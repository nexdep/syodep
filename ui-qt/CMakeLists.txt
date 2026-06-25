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
target_compile_definitions(syodep PRIVATE
    SYODEP_BUILD_TYPE="$<IF:$<BOOL:$<CONFIG>>,$<CONFIG>,unknown>")

set_target_properties(syodep PROPERTIES
    WIN32_EXECUTABLE TRUE)
