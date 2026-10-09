"""Unit tests for enrollment prompts, progress and safe retry behavior."""
import contextlib
import importlib.util
import io
from pathlib import Path
import unittest
from unittest.mock import Mock, patch

spec = importlib.util.spec_from_file_location("setup_nanacoin", Path(__file__).with_name("setup-nanacoin.py"))
setup = importlib.util.module_from_spec(spec)
spec.loader.exec_module(setup)


def answer(*, running=False, error=None, state="connected"):
    response = Mock()
    response.json.return_value = {"running": running, "error": error, "rows": [{"username": "trader_1", "state": state}]}
    return response


class EnrollmentCommandTests(unittest.TestCase):
    def invoke(self, client, **kwargs):
        with patch.object(setup.getpass, "getpass", side_effect=["nana secret", "owner secret"]), contextlib.redirect_stdout(io.StringIO()) as output:
            setup.run(client, setup.default_plan(), nana="nana", admin="owner", replace_read=False, yes=True, **kwargs)
        return output.getvalue()

    def test_defaults_are_distinct_and_have_no_grants(self):
        rows = setup.default_plan()["bots"]
        self.assertEqual(len(rows), 7)
        self.assertEqual(len({r["username"] for r in rows}), 7)
        self.assertEqual(len({r["strategy"] for r in rows[1:]}), 6)
        self.assertTrue(all(not r["initial_grant"] for r in rows))

    def test_cancel_never_prompts_for_passwords_or_contacts_server(self):
        client = Mock()
        with patch("builtins.input", return_value="n"), patch.object(setup.getpass, "getpass") as password, contextlib.redirect_stdout(io.StringIO()):
            setup.run(client, setup.default_plan(), nana="nana", admin="owner", replace_read=False, yes=False)
        password.assert_not_called()
        client.post.assert_not_called()

    def test_progress_polls_and_clears_credentials_without_printing_them(self):
        client = Mock()
        sent = []
        def post(path, *, json):
            self.assertEqual(path, "/api/v1/integrations/nanacoin/setup")
            self.assertEqual(json["nana_password"], "nana secret")
            self.assertFalse(json["replace_news_read_key"])
            sent.append(json)
            return answer(running=True, state="pending")
        client.post.side_effect = post
        client.get.return_value = answer()
        with patch.object(setup.time, "sleep"):
            output = self.invoke(client)
        self.assertEqual(sent, [{}])
        client.get.assert_called_once_with("/api/v1/integrations/nanacoin/setup")
        self.assertIn("Connected", output)
        self.assertNotIn("secret", output)

    def test_worker_failure_suggests_resuming(self):
        client = Mock()
        client.post.return_value = answer(error="Storage unavailable", state="failed")
        with self.assertRaisesRegex(SystemExit, "again to resume"):
            self.invoke(client)
        client.get.assert_not_called()

    def test_timeout_does_not_resubmit_accounts(self):
        client = Mock()
        client.post.return_value = answer(running=True)
        with patch.object(setup.time, "monotonic", side_effect=[0, 601]), self.assertRaisesRegex(SystemExit, "still running"):
            self.invoke(client)
        self.assertEqual(client.post.call_count, 1)
        client.get.assert_not_called()

    def test_http_error_also_clears_credentials(self):
        client = Mock()
        sent = []
        def fail(path, *, json):
            sent.append(json)
            raise OSError("connection failed")
        client.post.side_effect = fail
        with self.assertRaises(OSError):
            self.invoke(client)
        self.assertEqual(sent, [{}])


if __name__ == "__main__":
    unittest.main()
