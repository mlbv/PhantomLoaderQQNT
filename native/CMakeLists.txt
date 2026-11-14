# SPDX-FileCopyrightText: 2026 movblock
# SPDX-License-Identifier: LGPL-3.0-or-later

cmake_minimum_required(VERSION 3.24)
project(PhantomLoaderNativeFixtures LANGUAGES NONE)

option(BUILD_TESTING "Build optional local native fixtures" OFF)
if(BUILD_TESTING)
    if(NOT EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/tests/CMakeLists.txt")
        message(FATAL_ERROR "Local native fixture sources are unavailable")
    endif()
    enable_testing()
    add_subdirectory(tests)
else()
    message(STATUS "Production binaries are built with Cargo; enable BUILD_TESTING for local fixtures")
endif()
