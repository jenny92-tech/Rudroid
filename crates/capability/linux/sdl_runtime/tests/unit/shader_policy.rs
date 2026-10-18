//! INPUT:  immutable shader policies and GLSL source fixtures
//! OUTPUT:  conservative translation, opt-in refraction selection and repair assertions
//! POS:    safe policy checks independent of native GL context lifetime
use super::*;
#[test]
fn reference_gles2_translation_refuses_unsupported_io_and_multiple_outputs() {
    let p = Policy {
        gles2: true,
        ..Policy::default()
    };
    let source="#version 300 es\nlayout(location=0) in vec3 vertex;\nout vec2 uv;\nvoid main(){gl_Position=vec4(vertex,1.0);}";
    let v = p.rewrite(0x8b31, source).unwrap();
    assert!(
        v.source.contains("#version 100")
            && v.source.contains("attribute vec3")
            && v.source.contains("varying vec2")
    );
    for line in [
        "flat in vec2 uv;",
        "layout(std140) uniform Block { float x; };",
        "out vec4 first;\nout vec4 second;",
    ] {
        let text = format!("#version 300 es\n{line}\nvoid main(){{}}");
        assert!(p.rewrite(0x8b30, &text).is_none());
    }
}
#[test]
fn refraction_and_inverted_screen_sprite_are_opt_in_and_selected_exactly() {
    let p = Policy {
        refraction: true,
        skip: true,
        ..Policy::default()
    };
    let source =
        "uniform sampler2D _GrabTexture; uniform float _BumpAmt; void main(){ if(true){discard;} }";
    assert!(Policy::default().rewrite(0x8b30, source).is_none());
    let rewritten = p.rewrite(0x8b30, source).unwrap();
    assert!(rewritten.source.contains(MARKER));
    assert!(p.rewrite(0x8b31, source).is_none());
    assert!(p.rewrite(0x8b30, "void main(){ /* _BumpAmt */ }").is_none());
    let sprite = "_GrabTexture _MainTex (-u_xlat1.y) * 0.5 + 1.0";
    assert_eq!(
        p.rewrite(0x8b30, sprite).unwrap().source,
        "_GrabTexture _MainTex u_xlat1.y * 0.5"
    );
}

#[test]
fn all_preprocessor_mains_are_rewritten_and_comment_braces_or_markers_cannot_invent_skips() {
    let p = Policy {
        refraction: true,
        skip: true,
        ..Policy::default()
    };
    let source="uniform float _BumpAmt; uniform sampler2D _GrabTexture;\n// void main(){ }\n#if FIRST\nvoid main(){ /* } */ gl_FragColor=vec4(1.0); }\n#else\nvoid main(void){ gl_FragColor=vec4(0.0); }\n#endif";
    let result = p.rewrite(0x8b30, source).unwrap();
    assert!(result.skip);
    assert_eq!(result.source.matches("discard;").count(), 2);
    assert!(!result.source.contains("gl_FragColor="));
    let sprite = "_GrabTexture _MainTex (-u_xlat1.y) * 0.5 + 1.0 // RUDROID_SKIP_DRAW";
    assert!(!p.rewrite(0x8b30, sprite).unwrap().skip);
}
