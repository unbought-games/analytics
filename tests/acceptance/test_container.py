"""Build and boot the real image, migrate/bootstrap and retrieve MCP identity."""
import json
import os
from pathlib import Path
import unittest
from client import Client
from stack import load_state, start_engine


class ContainerTests(unittest.TestCase):
    def test_real_container_migration_runtime_bootstrap_and_mcp(self):
        state = load_state(os.environ["ANALYTICS_STACK_STATE"])
        start_engine(state)
        handoff = Path(state["directory"]) / "secrets/bootstrap.json"
        self.assertEqual(handoff.stat().st_mode & 0o777, 0o600)
        credentials = json.loads(handoff.read_text())
        client = Client(state["public_base_url"], credentials["credential"]["token"])
        client.initialize()
        self.assertEqual(client.tool("identity")["principal_id"], credentials["agent_id"])
        names = {tool["name"] for tool in client.rpc("tools/list")["tools"]}
        self.assertTrue({"admin_execute", "ingest_server", "export_create"}.issubset(names))
        self.assertTrue(client.tool("status")["ready"])


if __name__ == "__main__":
    unittest.main()
