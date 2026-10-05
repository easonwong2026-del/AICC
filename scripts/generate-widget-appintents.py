#!/usr/bin/env python3
import json
import os
import sys


def generate_metadata(output_dir, module_name="AICCWidget"):
    if module_name not in ("AICC", "AICCWidget"):
        raise ValueError("Expected AICC or AICCWidget module")
    prefix = f"{len(module_name)}{module_name}"
    config_type = prefix + ("29AICCWidgetConfigurationIntentV" if module_name == "AICC" else "0A19ConfigurationIntentV")
    os.makedirs(output_dir, exist_ok=True)

    version_data = {
        "version": "3.0",
        "toolsVersion": "17E6107"
    }
    with open(os.path.join(output_dir, "version.json"), "w") as f:
        json.dump(version_data, f, indent=2)
        f.write(chr(10))

    enum_metric_option = {
        "assistantDefinedSchemas": [],
        "availabilityAnnotations": {
            "LNPlatformNameWildcard": {
                "introducedVersion": "*"
            }
        },
        "cases": [
            {
                "displayRepresentation": {
                    "title": {
                        "alternatives": [],
                        "key": "Codex"
                    }
                },
                "identifier": "codex"
            },
            {
                "displayRepresentation": {
                    "title": {
                        "alternatives": [],
                        "key": "Google"
                    }
                },
                "identifier": "google"
            },
            {
                "displayRepresentation": {
                    "title": {
                        "alternatives": [],
                        "key": "WorkBuddy"
                    }
                },
                "identifier": "workbuddy"
            },
            {
                "displayRepresentation": {
                    "title": {
                        "alternatives": [],
                        "key": "DeepSeek"
                    }
                },
                "identifier": "deepseek"
            }
        ],
        "displayTypeName": {
            "alternatives": [],
            "key": "Metric"
        },
        "effectiveBundleIdentifiers": [],
        "fullyQualifiedTypeName": f"{module_name}.WidgetMetricOption",
        "identifier": "WidgetMetricOption",
        "isSystem": False,
        "mangledTypeName": prefix + "18WidgetMetricOptionO",
        "mangledTypeNameByBundleIdentifier": {},
        "visibilityMetadata": {
            "assistantOnly": False,
            "isDiscoverable": True
        }
    }

    def make_param(name, title, default_val):
        return {
            "capabilities": 1,
            "dynamicOptionsSupport": 0,
            "inputConnectionBehavior": 0,
            "isInput": False,
            "isOptional": False,
            "name": name,
            "parameterDescription": {
                "alternatives": [],
                "key": title
            },
            "resolvableInputTypes": [],
            "title": {
                "alternatives": [],
                "key": title
            },
            "typeSpecificMetadata": [
                "LNValueTypeSpecificMetadataKeyDefaultValue",
                {
                    "string": {
                        "wrapper": default_val
                    }
                }
            ],
            "valueType": {
                "linkEnumeration": {
                    "wrapper": {
                        "identifier": "WidgetMetricOption"
                    }
                }
            }
        }

    config_intent = {
        "actionConfiguration": {
            "whenClause": {
                "wrapper": {
                    "condition": {
                        "comparisonOperator": 2,
                        "parameterIdentifier": "system.widgetFamily",
                        "value": {
                            "string": {
                                "value": "systemSmall"
                            }
                        }
                    },
                    "when": {
                        "actionSummary": {
                            "wrapper": {
                                "otherParameterIdentifiers": [
                                    "primaryMetric",
                                    "secondaryMetric"
                                ]
                            }
                        }
                    },
                    "otherwise": {
                        "actionSummary": {
                            "wrapper": {
                                "otherParameterIdentifiers": [
                                    "topLeft",
                                    "topRight",
                                    "bottomLeft",
                                    "bottomRight"
                                ]
                            }
                        }
                    }
                }
            }
        },
        "assistantDefinedSchemaTraits": [],
        "assistantDefinedSchemas": [],
        "authenticationPolicy": 0,
        "availabilityAnnotations": {
            "LNPlatformNameWildcard": {
                "introducedVersion": "*"
            }
        },
        "descriptionMetadata": {
            "descriptionText": {
                "alternatives": [],
                "key": "Customize displayed metrics in AICC widget"
            },
            "searchKeywords": []
        },
        "effectiveBundleIdentifiers": [],
        "fullyQualifiedTypeName": f"{module_name}.AICCWidgetConfigurationIntent",
        "identifier": "AICCWidgetConfigurationIntent",
        "isAuthPolExplicit": False,
        "isDiscoverable": False,
        "mangledTypeName": config_type,
        "mangledTypeNameByBundleIdentifier": {},
        "mangledTypeNameByBundleIdentifierV2": {},
        "mangledTypeNameV2": config_type,
        "openAppWhenRun": True,
        "outputFlags": 1,
        "parameters": [
            make_param("primaryMetric", "Primary Metric", "codex"),
            make_param("secondaryMetric", "Secondary Metric", "google"),
            make_param("topLeft", "Top Left", "codex"),
            make_param("topRight", "Top Right", "google"),
            make_param("bottomLeft", "Bottom Left", "workbuddy"),
            make_param("bottomRight", "Bottom Right", "deepseek"),
        ],
        "presentationStyle": 0,
        "requiredCapabilities": [],
        "supportedModes": 2,
        "systemProtocolMetadata": [
            "com.apple.link.systemProtocol.WidgetConfiguration",
            {
                "empty": {}
            }
        ],
        "systemProtocolMetadataV2": [
            "com.apple.link.systemProtocol.WidgetConfiguration",
            {
                "empty": {}
            }
        ],
        "systemProtocols": [
            "com.apple.link.systemProtocol.WidgetConfiguration"
        ],
        "title": {
            "alternatives": [],
            "key": "AICC Widget Configuration"
        },
        "typeSpecificMetadata": [],
        "visibilityMetadata": {
            "assistantOnly": False,
            "isDiscoverable": False
        }
    }

    refresh_intent = {
        "assistantDefinedSchemaTraits": [],
        "assistantDefinedSchemas": [],
        "authenticationPolicy": 0,
        "availabilityAnnotations": {
            "LNPlatformNameWildcard": {
                "introducedVersion": "*"
            }
        },
        "descriptionMetadata": {
            "descriptionText": {
                "alternatives": [],
                "key": "Refreshes AICC collectors and loads their latest status."
            },
            "searchKeywords": []
        },
        "effectiveBundleIdentifiers": [],
        "fullyQualifiedTypeName": f"{module_name}.RefreshWidgetIntent",
        "identifier": "RefreshWidgetIntent",
        "isAuthPolExplicit": False,
        "isDiscoverable": True,
        "mangledTypeName": prefix + "19RefreshWidgetIntentV",
        "mangledTypeNameByBundleIdentifier": {},
        "mangledTypeNameByBundleIdentifierV2": {},
        "mangledTypeNameV2": prefix + "19RefreshWidgetIntentV",
        "openAppWhenRun": False,
        "outputFlags": 0,
        "parameters": [],
        "presentationStyle": 0,
        "requiredCapabilities": [],
        "supportedModes": 1,
        "systemProtocolMetadata": [],
        "systemProtocolMetadataV2": [],
        "systemProtocols": [],
        "title": {
            "alternatives": [],
            "key": "Refresh Widget"
        },
        "typeSpecificMetadata": [],
        "visibilityMetadata": {
            "assistantOnly": False,
            "isDiscoverable": True
        }
    }

    actionsdata = {
        "actions": {
            "AICCWidgetConfigurationIntent": config_intent,
            "RefreshWidgetIntent": refresh_intent
        },
        "assistantEntities": [],
        "assistantIntentNegativePhrases": [],
        "assistantIntents": [],
        "autoShortcuts": [],
        "entities": {},
        "enums": [enum_metric_option],
        "generator": {
            "name": "xcode-tools",
            "version": "17E6107"
        },
        "negativePhrases": [],
        "queries": {},
        "shortcutTileColor": 14,
        "version": 1
    }

    with open(os.path.join(output_dir, "extract.actionsdata"), "w") as f:
        json.dump(actionsdata, f, indent=2)
        f.write(chr(10))

    print(f"Generated App Intents metadata at {output_dir}")


if __name__ == "__main__":
    if len(sys.argv) < 2:
        print("Usage: generate-widget-appintents.py <output_metadata_dir> [AICC|AICCWidget]", file=sys.stderr)
        sys.exit(1)
    generate_metadata(sys.argv[1], sys.argv[2] if len(sys.argv) > 2 else "AICCWidget")
