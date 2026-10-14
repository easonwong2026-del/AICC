#!/bin/bash
set -euo pipefail
if ! timeout 240 ./android/poke-dashboard/gradlew -p android/poke-dashboard \
  -Pandroid.injected.androidTest.leaveApksInstalledAfterRun=true connectedDebugAndroidTest; then
  adb logcat -d -t 300
  exit 1
fi
adb pull /sdcard/Android/data/com.aieink.pokedashboard/files/visual android-visuals
