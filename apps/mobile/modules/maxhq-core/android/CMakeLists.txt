# SPDX-License-Identifier: MIT
# Copyright (c) 2026 Max Boksem

# Written by uniffi-bindgen-react-native and edited by hand, so `ubrn
# generate` leaves it be (see noOverwrite in ubrn.config.yaml).
cmake_minimum_required(VERSION 3.9.0)
project(MaxhqCore)

set (CMAKE_VERBOSE_MAKEFILE ON)
set (CMAKE_CXX_STANDARD 17)

# Resolve the path to the uniffi-bindgen-react-native package
execute_process(
    COMMAND node -p "require.resolve('uniffi-bindgen-react-native/package.json')"
    OUTPUT_VARIABLE UNIFFI_BINDGEN_PATH
    OUTPUT_STRIP_TRAILING_WHITESPACE
)
# Get the directory; get_filename_component and cmake_path will normalize
# paths with Windows path separators.
get_filename_component(UNIFFI_BINDGEN_PATH "${UNIFFI_BINDGEN_PATH}" DIRECTORY)

# Specifies a path to native header files.
include_directories(
    ../cpp
    ../cpp/generated

    ${UNIFFI_BINDGEN_PATH}/cpp/includes
)

add_library(maxhq-core            SHARED
    ../cpp/maxhq-core.cpp
    ../cpp/generated/maxhq_mobile.cpp
    cpp-adapter.cpp
)

# Set C++ compiler flags
set(CMAKE_CXX_FLAGS "${CMAKE_CXX_FLAGS} -O2")
set(CMAKE_CXX_FLAGS "${CMAKE_CXX_FLAGS} -fexceptions")
set(CMAKE_CXX_FLAGS "${CMAKE_CXX_FLAGS} -frtti")
set(CMAKE_CXX_FLAGS "${CMAKE_CXX_FLAGS} -fstack-protector-all")
set(CMAKE_CXX_FLAGS "${CMAKE_CXX_FLAGS} -Wall")
set(CMAKE_CXX_FLAGS "${CMAKE_CXX_FLAGS} -Werror")

# Set linker flags for 16KB page size alignment (required for Android 15+)
set(CMAKE_SHARED_LINKER_FLAGS "${CMAKE_SHARED_LINKER_FLAGS} -Wl,-z,max-page-size=16384")

# The core's JNI_OnLoad, which hands the key store to Rust, sits in the
# static library. Nothing here calls it, so the linker would leave it out;
# -u keeps it, and the VM calls it as the module loads the library.
set(CMAKE_SHARED_LINKER_FLAGS "${CMAKE_SHARED_LINKER_FLAGS} -Wl,-u,JNI_OnLoad")

cmake_path(
  SET MY_RUST_LIB
  ${CMAKE_SOURCE_DIR}/src/main/jniLibs/${ANDROID_ABI}/libmaxhq_mobile.a
  NORMALIZE
)
add_library(my_rust_lib STATIC IMPORTED)
set_target_properties(my_rust_lib PROPERTIES IMPORTED_LOCATION ${MY_RUST_LIB})

# Add ReactAndroid libraries, being careful to account for different versions.
find_package(ReactAndroid REQUIRED CONFIG)
find_library(LOGCAT log)

# REACTNATIVE_MERGED_SO seems to be only be set in a build.gradle.kt file,
# which we don't use. Thus falling back to version number sniffing.
if (ReactAndroid_VERSION_MINOR GREATER_EQUAL 76)
  set(REACTNATIVE_MERGED_SO true)
endif()

# https://github.com/react-native-community/discussions-and-proposals/discussions/816
# This if-then-else can be removed once this library does not support version below 0.76
if (REACTNATIVE_MERGED_SO)
  target_link_libraries(maxhq-core ReactAndroid::reactnative)
else()
  target_link_libraries(maxhq-core
    ReactAndroid::turbomodulejsijni
    ReactAndroid::react_nativemodule_core
  )
endif()

find_package(fbjni REQUIRED CONFIG)
target_link_libraries(
  maxhq-core
  fbjni::fbjni
  ReactAndroid::jsi
  ${LOGCAT}
  my_rust_lib
)
