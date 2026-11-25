//! INPUT: newly created VM session and its native method implementations
//! OUTPUT: core class identities and their native method registrations
//! POS: bootstrap of the shared VM before framework capabilities join it
use super::{class_lookup, shims, streams, JniNativeMethod, Session};
use std::ffi::{c_void, c_int};

pub(super) fn install_builtin_types(session: &Session) {
    // Establish Object before any other type receives its default parent.
    let object = session.find_class("java/lang/Object").unwrap();
    session.delete_ref(object, false, "bootstrap root type");
    class_lookup::bootstrap_exceptions(session);
    // Plugin metadata enters the same class authority through a normal JNI
    // native method. This internal bootstrap class owns no parallel state.
    let class = session.find_class("org/rudroid/runtime/TypeRegistry").unwrap();
    let methods = [
    JniNativeMethod { name:c"declareClass".as_ptr(), signature:c"(Ljava/lang/String;)Ljava/lang/Class;".as_ptr(), fn_ptr:class_lookup::declare_class as *mut c_void },
    JniNativeMethod { name:c"installClassCatalog".as_ptr(), signature:c"([Ljava/lang/String;)Z".as_ptr(), fn_ptr:class_lookup::install_catalog as *mut c_void },
    JniNativeMethod {
        name: c"setSuperclass".as_ptr(),
        signature: c"(Ljava/lang/Class;Ljava/lang/Class;)V".as_ptr(),
        fn_ptr: shims::set_superclass as *mut c_void,
    }, JniNativeMethod { name:c"defineField".as_ptr(), signature:c"(Ljava/lang/Class;Ljava/lang/String;Ljava/lang/String;Z)V".as_ptr(), fn_ptr:shims::define_field as *mut c_void },
    JniNativeMethod { name:c"proxyClass".as_ptr(), signature:c"(Ljava/lang/Class;[Ljava/lang/Class;)Ljava/lang/Class;".as_ptr(), fn_ptr:shims::proxy_class as *mut c_void },JniNativeMethod{name:c"fieldSignature".as_ptr(),signature:c"(Ljava/lang/reflect/Field;)Ljava/lang/String;".as_ptr(),fn_ptr:shims::field_signature as *mut c_void},
    JniNativeMethod{name:c"reflectField".as_ptr(),signature:c"(Ljava/lang/Class;Ljava/lang/String;Ljava/lang/String;Z)Ljava/lang/reflect/Field;".as_ptr(),fn_ptr:shims::lookup_reflected_field as *mut c_void},
    JniNativeMethod{name:c"reflectMethod".as_ptr(),signature:c"(Ljava/lang/Class;Ljava/lang/String;Ljava/lang/String;Z)Ljava/lang/reflect/Method;".as_ptr(),fn_ptr:shims::lookup_reflected_method as *mut c_void}];
    // SAFETY: static names/descriptors and shims matching each descriptor.
    unsafe { session.register_natives(class, methods.as_ptr(), methods.len() as i32); }
    session.delete_ref(class, false, "bootstrap types");
    let boolean=session.find_class("java/lang/Boolean").unwrap();
    session.define_field(session.resolve_class(boolean,"bootstrap Boolean").unwrap(),"value","Z",false);
    session.delete_ref(boolean,false,"bootstrap Boolean");
    let long=session.find_class("java/lang/Long").unwrap();session.define_field(session.resolve_class(long,"bootstrap Long").unwrap(),"value","J",false);session.delete_ref(long,false,"bootstrap Long");
    let integer=session.find_class("java/lang/Integer").unwrap();session.define_field(session.resolve_class(integer,"bootstrap Integer").unwrap(),"value","I",false);session.delete_ref(integer,false,"bootstrap Integer");
    for (name, bindings) in [
        ("java/nio/ByteBuffer",vec![JniNativeMethod{name:c"allocateDirect".as_ptr(),signature:c"(I)Ljava/nio/ByteBuffer;".as_ptr(),fn_ptr:shims::allocate_direct as *mut c_void},JniNativeMethod{name:c"_rudroidZeroOwned".as_ptr(),signature:c"()V".as_ptr(),fn_ptr:shims::zero_owned as *mut c_void}]),
        ("java/lang/System",vec![JniNativeMethod{name:c"identityHashCode".as_ptr(),signature:c"(Ljava/lang/Object;)I".as_ptr(),fn_ptr:shims::identity_hash as *mut c_void}]),
        ("java/lang/Long",vec![JniNativeMethod{name:c"<init>".as_ptr(),signature:c"(J)V".as_ptr(),fn_ptr:shims::long_init as *mut c_void},JniNativeMethod{name:c"longValue".as_ptr(),signature:c"()J".as_ptr(),fn_ptr:shims::long_value as *mut c_void}]),
        ("java/lang/Integer",vec![JniNativeMethod{name:c"<init>".as_ptr(),signature:c"(I)V".as_ptr(),fn_ptr:shims::integer_init as *mut c_void},JniNativeMethod{name:c"intValue".as_ptr(),signature:c"()I".as_ptr(),fn_ptr:shims::integer_value as *mut c_void},JniNativeMethod{name:c"parseInt".as_ptr(),signature:c"(Ljava/lang/String;)I".as_ptr(),fn_ptr:shims::integer_parse as *mut c_void}]),
        ("java/lang/Boolean",vec![
            JniNativeMethod{name:c"<init>".as_ptr(),signature:c"(Z)V".as_ptr(),fn_ptr:shims::boolean_init as *mut c_void},
            JniNativeMethod{name:c"booleanValue".as_ptr(),signature:c"()Z".as_ptr(),fn_ptr:shims::boolean_value as *mut c_void},
        ]),
        ("java/lang/Class", vec![
            // Java reflection shares the registered ordinary-class parents
            // with JNI GetSuperclass; this does not infer missing DEX kinds.
            JniNativeMethod { name:c"getSuperclass".as_ptr(), signature:c"()Ljava/lang/Class;".as_ptr(), fn_ptr:shims::get_superclass as *mut c_void },
            JniNativeMethod { name:c"isArray".as_ptr(), signature:c"()Z".as_ptr(), fn_ptr:shims::class_is_array as *mut c_void },
            JniNativeMethod { name:c"getClassLoader".as_ptr(), signature:c"()Ljava/lang/ClassLoader;".as_ptr(), fn_ptr:shims::class_loader as *mut c_void },
            JniNativeMethod { name:c"getName".as_ptr(), signature:c"()Ljava/lang/String;".as_ptr(), fn_ptr:shims::class_name_string as *mut c_void },
            JniNativeMethod { name:c"forName".as_ptr(), signature:c"(Ljava/lang/String;ZLjava/lang/ClassLoader;)Ljava/lang/Class;".as_ptr(), fn_ptr:shims::class_for_name as *mut c_void },
        ]),
        ("java/lang/reflect/Method", vec![
            JniNativeMethod { name:c"getName".as_ptr(),signature:c"()Ljava/lang/String;".as_ptr(),fn_ptr:shims::reflected_method_name as *mut c_void },
            JniNativeMethod { name:c"getDeclaringClass".as_ptr(),signature:c"()Ljava/lang/Class;".as_ptr(),fn_ptr:shims::reflected_declaring_class as *mut c_void },
        ]),
        ("java/lang/reflect/Constructor",vec![
            JniNativeMethod{name:c"getName".as_ptr(),signature:c"()Ljava/lang/String;".as_ptr(),fn_ptr:shims::reflected_declaring_name as *mut c_void},
            JniNativeMethod{name:c"getDeclaringClass".as_ptr(),signature:c"()Ljava/lang/Class;".as_ptr(),fn_ptr:shims::reflected_declaring_class as *mut c_void},
        ]),
        ("java/lang/reflect/Field",vec![JniNativeMethod{name:c"getName".as_ptr(),signature:c"()Ljava/lang/String;".as_ptr(),fn_ptr:shims::field_name as *mut c_void},JniNativeMethod{name:c"getDeclaringClass".as_ptr(),signature:c"()Ljava/lang/Class;".as_ptr(),fn_ptr:shims::field_declaring_class as *mut c_void}]),
        ("java/util/Scanner", vec![
            JniNativeMethod{name:c"<init>".as_ptr(),signature:c"(Ljava/io/InputStream;Ljava/lang/String;)V".as_ptr(),fn_ptr:streams::scanner::init as *mut c_void},
            JniNativeMethod{name:c"useDelimiter".as_ptr(),signature:c"(Ljava/lang/String;)Ljava/util/Scanner;".as_ptr(),fn_ptr:streams::scanner::delimiter as *mut c_void},
            JniNativeMethod{name:c"next".as_ptr(),signature:c"()Ljava/lang/String;".as_ptr(),fn_ptr:streams::scanner::next as *mut c_void},
            JniNativeMethod{name:c"hasNext".as_ptr(),signature:c"()Z".as_ptr(),fn_ptr:streams::scanner::has_next as *mut c_void},
            JniNativeMethod{name:c"close".as_ptr(),signature:c"()V".as_ptr(),fn_ptr:streams::scanner::close as *mut c_void},
        ]),
        ("java/io/FileInputStream", vec![
            JniNativeMethod{name:c"<init>".as_ptr(),signature:c"(Ljava/lang/String;)V".as_ptr(),fn_ptr:shims::stream_init as *mut c_void},
            JniNativeMethod{name:c"read".as_ptr(),signature:c"([B)I".as_ptr(),fn_ptr:shims::stream_read as *mut c_void},
            JniNativeMethod{name:c"read".as_ptr(),signature:c"([BII)I".as_ptr(),fn_ptr:shims::stream_read_range as *mut c_void},
            JniNativeMethod{name:c"available".as_ptr(),signature:c"()I".as_ptr(),fn_ptr:shims::stream_available as *mut c_void},
            JniNativeMethod{name:c"close".as_ptr(),signature:c"()V".as_ptr(),fn_ptr:shims::stream_close as *mut c_void},
        ]),
        ("java/lang/ClassLoader", vec![
            JniNativeMethod { name:c"loadClass".as_ptr(), signature:c"(Ljava/lang/String;)Ljava/lang/Class;".as_ptr(), fn_ptr:shims::load_class as *mut c_void },
            JniNativeMethod { name:c"findClass".as_ptr(), signature:c"(Ljava/lang/String;)Ljava/lang/Class;".as_ptr(), fn_ptr:shims::load_class as *mut c_void },
        ]),
        ("java/lang/String", vec![
            JniNativeMethod{name:c"<init>".as_ptr(),signature:c"()V".as_ptr(),fn_ptr:shims::immutable_string_constructor as *mut c_void},
            JniNativeMethod{name:c"<init>".as_ptr(),signature:c"([BLjava/lang/String;)V".as_ptr(),fn_ptr:shims::immutable_string_constructor as *mut c_void},
            JniNativeMethod{name:c"equals".as_ptr(),signature:c"(Ljava/lang/Object;)Z".as_ptr(),fn_ptr:shims::string_equals as *mut c_void},
            JniNativeMethod{name:c"isEmpty".as_ptr(),signature:c"()Z".as_ptr(),fn_ptr:shims::string_empty as *mut c_void},
            JniNativeMethod{name:c"length".as_ptr(),signature:c"()I".as_ptr(),fn_ptr:shims::get_string_length as *mut c_void},
            JniNativeMethod{name:c"getBytes".as_ptr(),signature:c"(Ljava/lang/String;)[B".as_ptr(),fn_ptr:shims::string_bytes_utf8 as *mut c_void},
        ]),
        ("java/lang/Object", vec![
            JniNativeMethod{name:c"equals".as_ptr(),signature:c"(Ljava/lang/Object;)Z".as_ptr(),fn_ptr:shims::object_equals as *mut c_void},
            JniNativeMethod { name:c"toString".as_ptr(), signature:c"()Ljava/lang/String;".as_ptr(), fn_ptr:shims::object_to_string as *mut c_void },
            JniNativeMethod { name:c"getClass".as_ptr(),signature:c"()Ljava/lang/Class;".as_ptr(),fn_ptr:shims::get_object_class as *mut c_void },
        ]),
        ("java/lang/StringBuilder", vec![
            JniNativeMethod { name:c"<init>".as_ptr(), signature:c"()V".as_ptr(), fn_ptr:shims::builder_init as *mut c_void },
            JniNativeMethod { name:c"append".as_ptr(), signature:c"(Ljava/lang/String;)Ljava/lang/StringBuilder;".as_ptr(), fn_ptr:shims::builder_append as *mut c_void },
            JniNativeMethod { name:c"toString".as_ptr(), signature:c"()Ljava/lang/String;".as_ptr(), fn_ptr:shims::builder_string as *mut c_void },
        ]),
    ] {
        let class = session.find_class(name).unwrap();
        // SAFETY: as the TypeRegistry bindings above.
        unsafe { session.register_natives(class, bindings.as_ptr(), bindings.len() as c_int); }
        session.delete_ref(class, false, "bootstrap types");
    }
    let stream=session.find_class("java/io/InputStream").unwrap();let file=session.find_class("java/io/FileInputStream").unwrap();session.set_superclass(file,stream);session.delete_ref(file,false,"bootstrap stream");session.delete_ref(stream,false,"bootstrap stream");
    let mut classes = session.classes.write().unwrap();
    let object = classes.by_name["java/lang/Object"];
    for name in ["java/lang/Class", "java/lang/ClassLoader", "java/lang/StringBuilder", "org/rudroid/runtime/TypeRegistry"] {
        let child = classes.by_name[name];
        classes.parents.insert(child, object);
    }
}
