source env.sh
cargo ndk -t arm64-v8a -o android/src/main/jniLibs/ -p 30 build --package n_player --lib --no-default-features