# SPDX-FileCopyrightText: 2026 movblock
# SPDX-License-Identifier: LGPL-3.0-or-later

cmake_minimum_required(VERSION 3.24)
project(QqntHookExample LANGUAGES CXX)

if(NOT WIN32 OR NOT MSVC OR NOT CMAKE_SIZEOF_VOID_P EQUAL 8)
    message(FATAL_ERROR "This example requires Windows x64 and MSVC")
endif()
set(NODE_INCLUDE_DIR "" CACHE PATH "Directory containing node_api.h")
if(NOT EXISTS "${NODE_INCLUDE_DIR}/node_api.h")
    message(FATAL_ERROR "Set NODE_INCLUDE_DIR to your Node-API header directory")
endif()

add_library(qqnt_hook SHARED addon.cpp)
target_include_directories(qqnt_hook PRIVATE "${NODE_INCLUDE_DIR}")
target_compile_features(qqnt_hook PRIVATE cxx_std_17)
target_compile_definitions(qqnt_hook PRIVATE NAPI_VERSION=8 WIN32_LEAN_AND_MEAN NOMINMAX)
target_compile_options(qqnt_hook PRIVATE /W4 /permissive-)
set_target_properties(qqnt_hook PROPERTIES
    PREFIX "" SUFFIX ".node"
    MSVC_RUNTIME_LIBRARY "MultiThreaded$<$<CONFIG:Debug>:Debug>")
