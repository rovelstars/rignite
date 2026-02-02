#!/bin/bash
export JAVA_HOME="/home/ren/coding/rovelos/Rignite/android_app/local_jdk"
export PATH="${JAVA_HOME}/bin:${PATH}"
./gradlew assembleDebug
