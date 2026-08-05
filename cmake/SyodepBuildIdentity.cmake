# Format the user-visible application version from the Cargo base version and
# the distribution channel. Keep this pure so every channel can be exercised
# with `cmake -P scripts/test-build-identity.cmake` on every platform.
function(syodep_format_build_version base_version channel commit out_var)
    if(NOT base_version MATCHES
            "^([0-9]+\\.[0-9]+\\.[0-9]+)(-([0-9A-Za-z.-]+))?(\\+([0-9A-Za-z.-]+))?$")
        message(FATAL_ERROR
            "Cargo version '${base_version}' is not a supported SemVer version")
    endif()
    set(numeric_version "${CMAKE_MATCH_1}")
    set(base_prerelease "${CMAKE_MATCH_3}")
    set(base_metadata "${CMAKE_MATCH_5}")

    if(channel STREQUAL "release")
        set(version "${base_version}")
    elseif(channel STREQUAL "continuous"
            OR channel STREQUAL "preview"
            OR channel STREQUAL "development")
        if(NOT commit MATCHES "^[0-9A-Za-z-]+$")
            message(FATAL_ERROR
                "Build commit '${commit}' is not a valid SemVer identifier")
        endif()

        if(channel STREQUAL "development")
            set(channel_label "dev")
        else()
            set(channel_label "${channel}")
        endif()

        if(NOT base_prerelease STREQUAL "")
            set(prerelease "${base_prerelease}.${channel_label}")
        else()
            set(prerelease "${channel_label}")
        endif()
        if(NOT base_metadata STREQUAL "")
            set(metadata "${base_metadata}.${commit}")
        else()
            set(metadata "${commit}")
        endif()
        set(version "${numeric_version}-${prerelease}+${metadata}")
    else()
        message(FATAL_ERROR
            "Unknown syodep build channel '${channel}'")
    endif()

    set(${out_var} "${version}" PARENT_SCOPE)
endfunction()
