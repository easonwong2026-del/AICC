#!/usr/bin/env python3
import json
import os
import sys

def generate_metadata(output_dir):
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
        "fullyQualifiedTypeName": "AICCWidget.WidgetMetricOption",
        "identifier": "WidgetMetricOption",
        "isSystem": False,
        "mangledTypeName": "10AICCWidget18WidgetMetricOptionO",
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
        "fullyQualifiedTypeName": "AICCWidget.AICCWidgetConfigurationIntent",
        "identifier": "AICCWidgetConfigurationIntent",
        "isAuthPolExplicit": False,
        "isDiscoverable": False,
        "mangledTypeName": "10AICCWidget0A19ConfigurationIntentV",
        "mangledTypeNameByBundleIdentifier": {},
        "mangledTypeNameByBundleIdentifierV2": {},
        "mangledTypeNameV2": "10AICCWidget0A19ConfigurationIntentV",
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
        "fullyQualifiedTypeName": "AICCWidget.RefreshWidgetIntent",
        "identifier": "RefreshWidgetIntent",
        "isAuthPolExplicit": False,
        "isDiscoverable": True,
        "mangledTypeName": "10AICCWidget19RefreshWidgetIntentV",
        "mangledTypeNameByBundleIdentifier": {},
        "mangledTypeNameByBundleIdentifierV2": {},
        "mangledTypeNameV2": "10AICCWidget19RefreshWidgetIntentV",
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
        print("Usage: generate-widget-appintents.py <output_metadata_dir>", file=sys.stderr)
        sys.exit(1)
    generate_metadata(sys.argv[1])
