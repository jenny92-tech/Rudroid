# INPUT:  The pinned converter's read-only desktop or Android build survey
# OUTPUT: ARM64 Rudroid route, Player profile evidence and native dependencies to review
# POS:    Host-side port_check hook, independent of title recipes and launch configuration

# A matching engine version is useful evidence for choosing a Player profile.
# It cannot identify the input game's assemblies, native plugins or device.
# Reference builds are recorded in crates/docs/{ACCEPTANCE,UNITY-MONO}.md.
REFERENCE_BUILDS = {
    ("mono", "6000.0.66f2"): "Kingdom Two Crowns",
    ("il2cpp", "2020.2.2f1"): "Hollow Knight",
    ("il2cpp", "2021.3.14f1c1"): "Black Myth Pixel",
    ("il2cpp", "2021.3.56f2"): "Terraria",
    ("il2cpp", "6000.0.58f1"): "Hollow Knight: Silksong",
    ("il2cpp", "6000.0.61f1"): "Vampire Survivors",
}

PLAYER_PROFILES = {
    "2020.2": "unity_2021_3",
    "2021.3": "unity_2021_3",
    "2022.3": "unity_2022_3",
    "6000.0": "unity_6",
}


def port_check(report):
    """Append our assessment without changing the converter's choice or exit code."""
    backend = report.get("backend")
    version = report.get("unity_version")
    build = report.get("build")
    code = report.get("code") or {}
    apk = report.get("apk_info") or {}
    routes = {entry["route"]: entry for entry in report.get("routes") or [] if isinstance(entry, dict) and "route" in entry}
    profile = PLAYER_PROFILES.get(report.get("unity_line"))
    reference = REFERENCE_BUILDS.get((backend, version))

    route = None
    reasons = []
    requirements = []
    if build == "android":
        candidate = routes.get("apk", {})
        if not candidate.get("ok"):
            reasons.extend(candidate.get("why") or ["Android package could not be inspected."])
        elif apk.get("abi") != "arm64-v8a":
            reasons.append("No inspected ARM64 Android Player and script backend.")
        elif backend != "il2cpp":
            reasons.append("Direct APK loading is established for ARM64 IL2CPP. Android Mono needs separate validation.")
        else:
            route = "apk"
            requirements.append("Stage the matching base/splits, game libraries, IL2CPP metadata and required assets.")
    elif build in {"linux", "macos", "windows"} and backend == "mono":
        candidate = routes.get("mono-arm64", {})
        if not candidate.get("ok"):
            reasons.extend(candidate.get("why") or ["No eligible ARM64 Mono conversion route was reported."])
        elif not version or not report.get("scripting_runtime") or not code.get("assemblies"):
            reasons.append("Confirm the Unity version, scripting runtime and readable game assemblies before preparation.")
        else:
            route = "mono-arm64"
            requirements.append("Use prepare with a matching ARM64 Android Player. Supply unity_mono, bridge, JIT and game DLLs.")
    else:
        reasons.append("This input has no established native ARM64 route from the detected build and script backend.")

    if route and not profile:
        requirements.append("Verify the Player JNI/lifecycle profile for this Unity line.")
    if route:
        requirements.append("Verify native dependencies, graphics, input and save/reload on the target device.")

    imports = code.get("pinvoke") if code.get("assemblies") else None
    if build == "android":
        plugins = apk.get("plugins")
    else:
        raw_plugins = report.get("plugins")
        plugins = [path for path, _ in raw_plugins] if raw_plugins is not None else None
    result = {
        "target_abi": "arm64-v8a",
        "route": route,
        "player_profile_candidate": profile if route else None,
        "reference_title_same_engine_and_backend": reference if route else None,
        "reasons": reasons,
        "requirements": requirements,
        "pinvoke_modules": imports,
        "unreadable_assemblies": code.get("unreadable"),
        "native_plugins": plugins,
    }
    report["rudroid"] = result
    notes = report.setdefault("notes", [])
    notes.append("Rudroid ARM64 preflight: " + (route or "no candidate") + ". " + " ".join(reasons + requirements))
    if route:
        notes.append(f"Rudroid Player profile candidate: {profile or 'unconfirmed'}. "
                     f"Device reference for this Unity version/backend: {reference or 'none recorded'}.")
    if imports:
        notes.append("Rudroid P/Invoke declarations to review for ARM64 libraries or explicit mappings: "
                     + ", ".join(sorted(imports)) + ". Actual calls depend on gameplay.")
    elif imports is None and backend == "mono":
        notes.append("Rudroid P/Invoke inventory unavailable: no readable game assemblies were surveyed.")
    if code.get("unreadable"):
        notes.append("Rudroid P/Invoke inventory is incomplete; some assemblies could not be read.")
    if plugins:
        notes.append("Rudroid native plugins to review for target ABI and dependencies: " + ", ".join(plugins))
