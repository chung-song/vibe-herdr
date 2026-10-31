import importlib.util
from pathlib import Path
import unittest

path = Path(__file__).resolve().parents[1] / "scripts" / "switch_dictation_mode.py"
spec = importlib.util.spec_from_file_location("switch_dictation_mode", path)
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class FakeDaemon:
    def __init__(self, on=True, phase="listening", reject=False):
        self.state = {"dictation": on, "dictation_mode": "auto", "dictation_phase": phase}
        self.reject = reject
        self.commands = []

    def __call__(self, command, **fields):
        self.commands.append(command)
        if command == "toggle":
            self.state["dictation"] = not self.state["dictation"]
        elif command == "cycle_dictation_mode":
            if self.reject:
                raise RuntimeError("permission denied")
            self.state["dictation_mode"] = "space" if self.state["dictation_mode"] == "auto" else "auto"
        return self.state.copy()


class SwitchModeTest(unittest.TestCase):
    def test_running_mode_switches_and_stays_on(self):
        daemon = FakeDaemon()
        module.switch(daemon, lambda _: None)
        self.assertTrue(daemon.state["dictation"])
        self.assertEqual(daemon.state["dictation_mode"], "space")
        self.assertEqual(daemon.commands, ["status", "status", "toggle", "cycle_dictation_mode", "status", "toggle"])

    def test_busy_audio_is_not_interrupted(self):
        daemon = FakeDaemon(phase="transcribing")
        with self.assertRaises(RuntimeError):
            module.switch(daemon, lambda _: None)
        self.assertEqual(daemon.commands, ["status"])
        self.assertTrue(daemon.state["dictation"])

    def test_failed_switch_restores_on_state(self):
        daemon = FakeDaemon(reject=True)
        with self.assertRaises(RuntimeError):
            module.switch(daemon, lambda _: None)
        self.assertTrue(daemon.state["dictation"])
        self.assertEqual(daemon.state["dictation_mode"], "auto")

    def test_off_state_stays_off(self):
        daemon = FakeDaemon(on=False)
        module.switch(daemon, lambda _: None)
        self.assertFalse(daemon.state["dictation"])
        self.assertEqual(daemon.state["dictation_mode"], "space")

    def test_space_mode_switches_back_to_auto(self):
        daemon = FakeDaemon()
        daemon.state["dictation_mode"] = "space"
        module.switch(daemon, lambda _: None)
        self.assertTrue(daemon.state["dictation"])
        self.assertEqual(daemon.state["dictation_mode"], "auto")


if __name__ == "__main__":
    unittest.main()
