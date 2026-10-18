# JNI fixture header

`jni.h` is a byte-for-byte copy of `libjnivm/include/jni.h` from Bogodroid
commit `2959635ef9bb64b605aad3db53287d4517b11a5b`. It retains the Android Open Source Project
copyright, Apache-2.0 notice and the existing source-navigation comments.
It is a vendored ABI oracle used by the C test probe, not the C++ JNI VM.

The probe includes this local header so normal Rust tests do not require
the sibling Bogodroid tree. Do not replace the header with guessed JNI layouts
or remove its third-party notices.
