# INPUT:  serialized Unity resource paths and a bundle's named nodes
# OUTPUT:  resource_basename(), internal_resource_name()
# POS:    matches UnityPy's basename lookup without silently accepting ambiguous owners
# SPDX-License-Identifier: LicenseRef-Rudroid-NC-SourceShare-1.0
import ntpath


def resource_basename(path):
    # UnityPy ResourceReader uses ntpath.basename for plain, Windows and
    # archive:/CAB/name references. Prefixes are not filesystem traversal.
    if not isinstance(path, str) or "\0" in path:
        raise ValueError(f"invalid resource path: {path!r}")
    name = ntpath.basename(path)
    if name in ("", ".", ".."):
        raise ValueError(f"invalid resource basename: {path!r}")
    return name


def internal_resource_name(bundle, path):
    name = resource_basename(path).lower()
    matches = [key for key in getattr(bundle, "files", {})
               if resource_basename(key).lower() == name]
    # Environment.register_cab silently replaces earlier basename matches.
    # A qualified source path cannot make that global decoder index unambiguous.
    if len(matches) > 1:
        raise ValueError(f"ambiguous internal resource owner: {path!r}")
    return matches[0] if matches else None
