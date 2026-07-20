# Keep only the manifest entry points; R8 can remove all other unused code.
-keep class com.aieink.pokedashboard.MainActivity { *; }
-keep class com.aieink.pokedashboard.BootReceiver { *; }
