cmake_minimum_required(VERSION 3.21)

include("${CMAKE_CURRENT_LIST_DIR}/../cmake/SyodepBuildIdentity.cmake")

function(assert_build_version expected base channel commit)
    syodep_format_build_version(
        "${base}" "${channel}" "${commit}" actual)
    if(NOT actual STREQUAL expected)
        message(FATAL_ERROR
            "${base} / ${channel} / ${commit}: expected '${expected}', got '${actual}'")
    endif()
endfunction()

assert_build_version("0.16.0" "0.16.0" "release" "abcdef123456")
assert_build_version(
    "0.16.0-continuous+abcdef123456"
    "0.16.0" "continuous" "abcdef123456")
assert_build_version(
    "0.16.0-preview+abcdef123456"
    "0.16.0" "preview" "abcdef123456")
assert_build_version(
    "0.16.0-dev+abcdef123456"
    "0.16.0" "development" "abcdef123456")

# Cargo prereleases remain prereleases in every channel. Channel information
# extends the existing prerelease identifiers rather than producing a second
# hyphen-delimited version that only looks SemVer-like.
assert_build_version("0.16.0-rc.1" "0.16.0-rc.1" "release" "abcdef123456")
assert_build_version(
    "0.16.0-rc.1.continuous+abcdef123456"
    "0.16.0-rc.1" "continuous" "abcdef123456")
assert_build_version(
    "0.16.0-preview+vendor.abcdef123456"
    "0.16.0+vendor" "preview" "abcdef123456")

message(STATUS "build identity tests OK")
