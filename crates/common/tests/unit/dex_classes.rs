use super::*;

fn put(data: &mut [u8], at: usize, value: usize) {
    data[at..at+4].copy_from_slice(&(value as u32).to_le_bytes());
}

fn fixture() -> Vec<u8> {
    fixture_with_name("Lsample/Defined例;")
}

fn fixture_with_name(name: &str) -> Vec<u8> {
    // Two type references, only the second has a class definition. The first
    // must stay absent even though a descriptor reference occurs in the DEX.
    let mut data = vec![0; 160];
    data[..8].copy_from_slice(b"dex\n035\0");
    put(&mut data,36,112);put(&mut data,40,0x12345678);
    for (at,value) in [(56,2),(60,112),(64,2),(68,120),(96,1),(100,128),(108,160)] {put(&mut data,at,value);}
    put(&mut data,120,0);put(&mut data,124,1);put(&mut data,128,1);
    for (index,text) in ["Lsample/Referenced;",name].iter().enumerate() {
        let at=data.len();put(&mut data,112+index*4,at);
        data.push(text.encode_utf16().count() as u8);
        data.extend_from_slice(text.as_bytes());data.push(0);
    }
    let size=data.len();put(&mut data,32,size);put(&mut data,104,size-160);
    data
}

#[test]
fn only_class_defs_count_not_type_references() {
    assert_eq!(defined_classes(&fixture()).unwrap(),["sample/Defined例"]);
}

#[test]
fn truncated_headers_tables_and_indices_reject_without_panicking() {
    let original=fixture();
    for length in [0,7,31,111,159,original.len()-1] {assert!(defined_classes(&original[..length]).is_err());}
    for (at,value) in [(32,0),(36,120),(40,0x78563412),(60,usize::MAX),(68,113),(96,MAX_CLASSES+1),(128,2),(124,2),(116,1),(108,0)] {
        let mut data=original.clone();put(&mut data,at,value);
        assert!(defined_classes(&data).is_err(),"word {at}");
    }
}

#[test]
fn unsupported_format_length_and_descriptor_syntax_reject() {
    let original=fixture();
    let mut data=original.clone();data[4..7].copy_from_slice(b"041");assert!(defined_classes(&data).is_err());
    let descriptor=word(&original,116).unwrap();
    for (at,value) in [(descriptor,1),(descriptor+1,b'['),(original.len()-1,b'x')] {
        let mut data=original.clone();data[at]=value;assert!(defined_classes(&data).is_err());
    }
    for name in ["","/A","A/","a//B","a.B","[La/B;","a\0B","a:B","a\\B","a B","a\u{a0}B"] {assert!(!valid_class_name(name));}
    assert!(valid_class_name("com/example/Outer$Nested_2"));
    assert!(defined_classes(&fixture_with_name("Lsample/😀;")).is_err());
}
