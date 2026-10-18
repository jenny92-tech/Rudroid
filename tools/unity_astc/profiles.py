# INPUT:  ASTC target texture capability and optional memory class
# OUTPUT:  DEFAULT_TARGET and TARGETS, the coarse-to-fine candidate lists
# POS:    separates target memory choices from codec and command-line implementation
# SPDX-License-Identifier: LicenseRef-Rudroid-NC-SourceShare-1.0
DEFAULT_TARGET = "astc-1g"
TARGETS = {"astc-1g": (12, 8, 6, 4), "astc-2g": (8, 6, 4), "astc-4g": (4,)}
