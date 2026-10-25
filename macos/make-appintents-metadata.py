#!/usr/bin/env python3
"""Write `Metadata.appintents` into the hand-built widget extension.

Xcode's `appintentsmetadataprocessor` build phase drops this directory into the
appex, and it is what lets the system resolve a `Button(intent:)` tap back to the
extension that owns the intent.  Without it the tap is silently dropped — the
button draws, nothing happens — which is exactly what a Command Line Tools only
build gets.  The payload is plain JSON, so it is generated here from the intent
names the extension compiles.

Called by `macos/build-aicc-swiftui.sh` before the appex is signed: the files
sit inside the sealed bundle, so they must exist first.
"""

import json
import pathlib
import sys

MODULE = "AICCWidget"

# identifier, title, description — must match the structs in BoardWidget.swift.
ACTIONS = [
    ("BoardStepIntent", "切换 AICC 看板页", "Move the AICC usage board one page"),
    ("BoardRefreshIntent", "刷新 AICC 看板", "Read Magpie for the AICC usage board"),
]


def mangled(identifier: str) -> str:
    """Swift mangling for `struct <identifier>` in this module: no `$s` prefix."""
    return f"{len(MODULE)}{MODULE}{len(identifier)}{identifier}V"


def action(identifier: str, title: str, description: str) -> dict:
    summary = {
        "wrapper": {
            "otherParameterIdentifiers": [],
            "summaryString": {"formatString": title, "parameterIdentifiers": []},
            "table": "AppIntents",
        }
    }
    return {
        "actionConfiguration": {"actionSummary": summary},
        "assistantDefinedSchemas": [],
        "assistantDefinedSchemaTraits": [],
        "authenticationPolicy": 0,
        "availabilityAnnotations": {"LNPlatformNameWildcard": {"introducedVersion": "*"}},
        "descriptionMetadata": {
            "categoryName": {"title": {"alternatives": [], "key": "BOARD", "table": "AppIntents"}},
            "descriptionText": {"alternatives": [], "key": description, "table": "AppIntents"},
            "searchKeywords": [],
        },
        "effectiveBundleIdentifiers": [],
        "fullyQualifiedTypeName": f"{MODULE}.{identifier}",
        "identifier": identifier,
        "isAuthPolExplicit": True,
        "isDiscoverable": False,
        "mangledTypeName": mangled(identifier),
        "mangledTypeNameByBundleIdentifier": {},
        "mangledTypeNameByBundleIdentifierV2": {},
        "mangledTypeNameV2": mangled(identifier),
        "openAppWhenRun": False,
        "outputFlags": 8,
        "parameters": [],
        "presentationStyle": 0,
        "requiredCapabilities": [],
        "sideEffect": {"changeEffect": -1, "effect": -1},
        "supportedModes": 1,
        "systemProtocolMetadata": [],
        "systemProtocolMetadataV2": [],
        "systemProtocols": [],
        "title": {"alternatives": [], "key": title},
        "typeSpecificMetadata": [],
    }


def main() -> int:
    if len(sys.argv) != 2:
        print("usage: make-appintents-metadata.py <AICCWidget.appex>", file=sys.stderr)
        return 2
    appex = pathlib.Path(sys.argv[1])
    out = appex / "Contents" / "Resources" / "Metadata.appintents"
    if not (appex / "Contents").is_dir():
        print(f"not an appex: {appex}", file=sys.stderr)
        return 1
    out.mkdir(parents=True, exist_ok=True)

    # The surrounding macOS ships 3.0; the tool version only tags the generator.
    (out / "version.json").write_text(
        json.dumps({"version": "3.0", "toolsVersion": "26.0"}, indent=2) + "\n",
        encoding="utf-8",
    )
    payload = {
        "actions": {ident: action(ident, title, description)
                    for ident, title, description in ACTIONS},
        "assistantEntities": [],
        "assistantIntentNegativePhrases": [],
        "assistantIntents": [],
        "autoShortcuts": [],
        "entities": {},
        "enums": [],
        "generator": {"name": "xcode-tools", "version": "26.0"},
        "negativePhrases": [],
        "queries": {},
        "shortcutTileColor": 14,
        "version": 1,
    }
    (out / "extract.actionsdata").write_text(
        json.dumps(payload, ensure_ascii=False, separators=(",", ":")), encoding="utf-8"
    )
    print(f"App Intents metadata: {len(ACTIONS)} actions -> {out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
