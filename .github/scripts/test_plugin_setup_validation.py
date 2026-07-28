#!/usr/bin/env python3

"""Keep the bundled plugin setup validator covered by existing repository CI."""

import importlib.util
from pathlib import Path
import sys
from types import ModuleType
from typing import Any
import unittest


def load_plugin_validator() -> ModuleType:
    if importlib.util.find_spec("yaml") is None:
        sys.modules["yaml"] = ModuleType("yaml")

    validator_path = (
        Path(__file__).resolve().parents[2]
        / "codex-rs"
        / "skills"
        / "src"
        / "assets"
        / "samples"
        / "plugin-creator"
        / "scripts"
        / "validate_plugin.py"
    )
    spec = importlib.util.spec_from_file_location(
        "codex_plugin_creator_setup_validator", validator_path
    )
    if spec is None or spec.loader is None:
        raise RuntimeError(f"could not load plugin validator at {validator_path}")
    validator = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(validator)
    return validator


VALIDATOR = load_plugin_validator()


def setup_errors(setup: Any) -> list[str]:
    errors: list[str] = []
    VALIDATOR.validate_manifest_setup({"setup": setup}, errors)
    return errors


def customer_setup() -> dict[str, Any]:
    return {
        "inputs": [
            {
                "id": "project_root",
                "type": "directory",
                "prompt": "Project directory",
                "env": "CUSTOMER_PROJECT_ROOT",
            },
            {
                "id": "api_key",
                "type": "secret",
                "prompt": "API key",
                "env": "CUSTOMER_API_KEY",
            },
        ],
        "commands": [
            {
                "name": "authenticate",
                "command": ["python3", "./scripts/authenticate.py"],
                "interactive": True,
            },
            {
                "name": "configure",
                "command": ["python3", "./scripts/configure.py"],
            },
        ],
    }


class PluginSetupValidatorTest(unittest.TestCase):
    def test_accepts_typed_inputs_and_ordered_interactive_commands(self) -> None:
        self.assertEqual(setup_errors(customer_setup()), [])

    def test_accepts_legacy_single_command(self) -> None:
        self.assertEqual(
            setup_errors({"command": ["python3", "./scripts/setup.py"]}), []
        )

    def test_rejects_ambiguous_command_forms(self) -> None:
        setup = customer_setup()
        setup["command"] = ["python3", "./scripts/legacy.py"]
        self.assertTrue(any("not both" in error for error in setup_errors(setup)))

    def test_rejects_unknown_setup_fields(self) -> None:
        setup = customer_setup()
        setup["silent"] = True
        self.assertTrue(any("silent" in error for error in setup_errors(setup)))

    def test_rejects_reserved_plugin_environment_variables(self) -> None:
        for variable in (
            "PLUGIN_ROOT",
            "PLUGIN_DATA",
            "CLAUDE_PLUGIN_ROOT",
            "CLAUDE_PLUGIN_DATA",
        ):
            with self.subTest(variable=variable):
                setup = customer_setup()
                setup["inputs"][0]["env"] = variable
                self.assertTrue(
                    any("reserved" in error for error in setup_errors(setup))
                )

    def test_rejects_duplicate_input_identifiers(self) -> None:
        setup = customer_setup()
        setup["inputs"][1]["id"] = setup["inputs"][0]["id"]
        self.assertTrue(any("more than once" in error for error in setup_errors(setup)))

    def test_rejects_duplicate_environment_variables(self) -> None:
        setup = customer_setup()
        setup["inputs"][1]["env"] = setup["inputs"][0]["env"]
        self.assertTrue(any("more than once" in error for error in setup_errors(setup)))

    def test_rejects_duplicate_command_names(self) -> None:
        setup = customer_setup()
        setup["commands"][1]["name"] = setup["commands"][0]["name"]
        self.assertTrue(any("more than once" in error for error in setup_errors(setup)))

    def test_rejects_terminal_control_sequences(self) -> None:
        setup = customer_setup()
        setup["inputs"][0]["prompt"] = "Project\x1b[2J directory"
        setup["commands"][0]["name"] = "Authenticate\x1b[2J"
        errors = setup_errors(setup)

        self.assertTrue(any("prompt" in error for error in errors))
        self.assertTrue(any("name" in error for error in errors))

    def test_rejects_oversized_rendered_command_plan(self) -> None:
        setup = customer_setup()
        setup["commands"] = [
            {"name": f"step-{index}", "command": ["python3", "x" * 1_500]}
            for index in range(3)
        ]
        self.assertTrue(any("command plan" in error for error in setup_errors(setup)))

    def test_rejects_excessive_command_count(self) -> None:
        setup = customer_setup()
        setup["commands"] = [
            {"name": f"step-{index}", "command": ["python3"]}
            for index in range(VALIDATOR.MAX_SETUP_COMMAND_COUNT + 1)
        ]
        self.assertTrue(any("more than" in error for error in setup_errors(setup)))

    def test_rejects_invalid_command_arguments_without_crashing(self) -> None:
        setup = customer_setup()
        setup["commands"][0]["command"] = ["python3", ["not-an-argument"]]

        self.assertTrue(any("command" in error for error in setup_errors(setup)))

    def test_rejects_missing_commands(self) -> None:
        self.assertTrue(
            any("at least one command" in error for error in setup_errors({}))
        )


if __name__ == "__main__":
    unittest.main()
