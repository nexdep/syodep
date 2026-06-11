find_package(Qt6 REQUIRED COMPONENTS Widgets OpenGLWidgets)

qt_standard_project_setup()

qt_add_executable(syodep
    src/main.cpp
    src/main_window.cpp
    src/main_window.h
    src/canvas_widget.cpp
    src/canvas_widget.h
    src/key_encoder.cpp
    src/key_encoder.h)

target_link_libraries(syodep PRIVATE
    Qt6::Widgets
    Qt6::OpenGLWidgets
    syodep_ffi)

set_target_properties(syodep PROPERTIES
    WIN32_EXECUTABLE TRUE)
