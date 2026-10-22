"""Build and boot the real image with automatic migrations, then bootstrap and use MCP."""
import json
import os
import socket
import subprocess
from pathlib import Path
import unittest
from client import Client
from stack import ROOT, load_state, start_engine, compose, endpoint


class ContainerTests(unittest.TestCase):
    def test_real_container_automatic_migrations_bootstrap_and_mcp(self):
        state = load_state(os.environ["ANALYTICS_STACK_STATE"])
        prior_id=compose(state,"ps","-q","fixtures",capture_output=True,text=True).stdout.strip()
        start_engine(state,recreate_dependencies=True)
        self.assertNotEqual(compose(state,"ps","-q","fixtures",capture_output=True,text=True).stdout.strip(),prior_id)
        self.assertEqual(state["public_base_url"],"http://"+endpoint(state,"fixtures",8080))
        handoff = Path(state["directory"]) / "secrets/bootstrap.json"
        self.assertEqual(handoff.stat().st_mode & 0o777, 0o600)
        credentials = json.loads(handoff.read_text())
        client = Client(state["public_base_url"], credentials["credential"]["token"])
        client.initialize()
        self.assertEqual(client.tool("identity")["principal_id"], credentials["agent_id"])
        names = {tool["name"] for tool in client.rpc("tools/list")["tools"]}
        self.assertTrue({"admin_execute", "ingest_server", "export_create"}.issubset(names))
        self.assertTrue(client.tool("status")["ready"])

    def test_standalone_deploy_compose_with_default_user_and_persistent_volume(self):
        state = load_state(os.environ["ANALYTICS_STACK_STATE"])
        compose(state, "exec", "-T", "postgres", "psql", "-X", "-U", "analytics_migration", "-d", "postgres", "-v", "ON_ERROR_STOP=1",
                input="CREATE DATABASE deploy_smoke OWNER analytics_runtime;", text=True, capture_output=True)
        directory = Path(state["directory"])
        with socket.socket() as sock:
            sock.bind(("127.0.0.1", 0))
            port = sock.getsockname()[1]
        url = f"http://127.0.0.1:{port}"
        # Only test network/port wiring differs from the documented deployment.
        override = directory / "deploy-override.yaml"
        override.write_text(f"services:\n  analytics:\n    ports: !override [\"127.0.0.1:{port}:8080\"]\nnetworks:\n  default:\n    external: true\n    name: {state['project']}_default\n")
        env = dict(os.environ, DATABASE_URL=(directory / "engine_database_url").read_text().rsplit("/", 1)[0] + "/deploy_smoke", ANALYTICS_PUBLIC_BASE_URL=url)
        project = state["project"] + "-deploy"
        command = ["docker", "compose", "--project-name", project, "--file", str(ROOT / "compose.deploy.yaml"), "--file", str(override)]
        def deploy(*args, check=True):
            return subprocess.run([*command, *args], env=env, cwd=ROOT, check=check, capture_output=True, text=True, timeout=300)
        try:
            deploy("up", "-d", "--build", "--wait", "--wait-timeout", "90")
            self.assertEqual(deploy("exec", "-T", "analytics", "id", "-u").stdout.strip(), "65532")
            deploy("exec", "-T", "analytics", "analytics-server", "bootstrap", "--owner", "Owner", "--agent", "Agent", "--output", "/var/lib/analytics/handoff/initial.json")
            handoff = directory / "deploy-bootstrap.json"
            deploy("cp", "analytics:/var/lib/analytics/handoff/initial.json", str(handoff))
            self.assertEqual(handoff.stat().st_mode & 0o777, 0o600)
            credentials = json.loads(handoff.read_text())
            client = Client(url, credentials["credential"]["token"])
            client.initialize()
            self.assertEqual(client.tool("identity")["principal_id"], credentials["agent_id"])
            deploy("up", "-d", "--force-recreate", "--wait", "--wait-timeout", "90")
            self.assertEqual(client.tool("identity")["principal_id"], credentials["agent_id"])
            duplicate = deploy("exec", "-T", "analytics", "analytics-server", "bootstrap", "--owner", "Owner", "--agent", "Agent", "--output", "/var/lib/analytics/handoff/duplicate.json", check=False)
            self.assertNotEqual(duplicate.returncode, 0)
            self.assertIn("already_initialized", duplicate.stderr)
        finally:
            deploy("down", "--volumes", "--remove-orphans", "--timeout", "10")
            compose(state, "exec", "-T", "postgres", "psql", "-X", "-U", "analytics_migration", "-d", "postgres", "-v", "ON_ERROR_STOP=1",
                    input="DROP DATABASE deploy_smoke WITH (FORCE);", text=True, capture_output=True)


if __name__ == "__main__":
    unittest.main()
