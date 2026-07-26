# pyright: reportMissingImports=false

import json
import os
import subprocess
import tempfile
import unittest
from http import HTTPStatus
from unittest import mock

import opencodex_proxy.app as app_mod

ProxyConfig = app_mod.ProxyConfig
ProxyError = app_mod.ProxyError
load_provider_config = app_mod.load_provider_config
resolve_api_key = app_mod.resolve_api_key
resolve_upstream_target = app_mod.resolve_upstream_target
visible_model_ids = app_mod.visible_model_ids
normalize_protocol = app_mod.normalize_protocol
upstream_endpoint = app_mod.upstream_endpoint
call_upstream_responses = app_mod.call_upstream_responses


def make_config() -> app_mod.ProxyConfig:
    return ProxyConfig(
        bind="127.0.0.1",
        port=8787,
        chat_base_url="https://opencode.ai/zen/go/v1",
        api_key_env="OPENCODE_GO_API_KEY",
        timeout_sec=1,
        max_body_bytes=20 * 1024 * 1024,
    )


class ResolveRouteTests(unittest.TestCase):
    def test_responses_provider_forwards_native_request(self) -> None:
        class FakeResponse:
            status = 200

            def __enter__(self):
                return self

            def __exit__(self, *_args):
                return False

            def read(self):
                return b'{"id":"resp_1","status":"completed","output":[]}'

        cfg = ProxyConfig(
            bind="127.0.0.1",
            port=8787,
            chat_base_url="",
            api_key_env="OPENCODE_GO_API_KEY",
            timeout_sec=1,
            max_body_bytes=20 * 1024 * 1024,
        )
        with mock.patch("urllib.request.urlopen", return_value=FakeResponse()) as urlopen:
            response = call_upstream_responses(
                {"model": "grok", "input": "hello"},
                cfg,
                "req",
                upstream={"base_url": "https://example.com/v1", "api_key": "key", "protocol": "responses"},
            )
        request = urlopen.call_args.args[0]
        self.assertEqual(request.full_url, "https://example.com/v1/responses")
        self.assertEqual(json.loads(request.data)["input"], "hello")
        self.assertEqual(response["status"], "completed")

    def test_provider_protocol_defaults_to_responses(self) -> None:
        config = {"providers": {"native": {"baseUrl": "https://example.com/v1", "models": ["grok"]}}}
        with tempfile.NamedTemporaryFile("w", suffix=".json") as file:
            json.dump(config, file)
            file.flush()
            providers, _mappings, _routes, _models = load_provider_config(file.name)
        self.assertEqual(providers["native"]["protocol"], "responses")

    def test_chat_completions_protocol_remains_available(self) -> None:
        config = {"providers": {"legacy": {"baseUrl": "https://example.com/v1", "protocol": "chat_completions"}}}
        with tempfile.NamedTemporaryFile("w", suffix=".json") as file:
            json.dump(config, file)
            file.flush()
            providers, _mappings, _routes, _models = load_provider_config(file.name)
        self.assertEqual(providers["legacy"]["protocol"], "chat_completions")

    def test_responses_endpoint_does_not_duplicate_suffix(self) -> None:
        self.assertEqual(upstream_endpoint("https://example.com/v1", "responses"), "https://example.com/v1/responses")
        self.assertEqual(upstream_endpoint("https://example.com/v1/responses", "responses"), "https://example.com/v1/responses")

    def test_colon_format_routes_to_provider(self) -> None:
        cfg = ProxyConfig(
            bind="127.0.0.1",
            port=8787,
            chat_base_url="https://opencode.ai/zen/go/v1",
            api_key_env="OPENCODE_GO_API_KEY",
            timeout_sec=1,
            max_body_bytes=20 * 1024 * 1024,
            providers={"Ollama": {"base_url": "https://ollama.com/v1", "api_key": "k"}},
        )
        pname, mname, base, key = cfg.resolve_route("Ollama:minimax-m2.5")
        self.assertEqual(pname, "Ollama")
        self.assertEqual(mname, "minimax-m2.5")
        self.assertEqual(base, "https://ollama.com/v1")
        self.assertEqual(key, "k")

    def test_colon_format_unknown_provider_falls_through(self) -> None:
        cfg = make_config()
        pname, mname, base, key = cfg.resolve_route("Unknown:model")
        self.assertEqual(pname, "default")

    def test_explicit_mapping_still_works(self) -> None:
        cfg = ProxyConfig(
            bind="127.0.0.1",
            port=8787,
            chat_base_url="https://opencode.ai/zen/go/v1",
            api_key_env="OPENCODE_GO_API_KEY",
            timeout_sec=1,
            max_body_bytes=20 * 1024 * 1024,
            providers={"Ollama": {"base_url": "https://ollama.com/v1", "api_key": "k"}},
            mappings={"gpt-5.5": "Ollama:minimax-m2.5"},
        )
        pname, mname, base, key = cfg.resolve_route("gpt-5.5")
        self.assertEqual(pname, "Ollama")
        self.assertEqual(mname, "minimax-m2.5")

    def test_no_default_upstream_is_unrouted(self) -> None:
        cfg = ProxyConfig(
            bind="127.0.0.1",
            port=8787,
            chat_base_url="",
            api_key_env="OPENCODE_GO_API_KEY",
            timeout_sec=1,
            max_body_bytes=20 * 1024 * 1024,
        )
        pname, mname, base, key = cfg.resolve_route("deepseek-v4-flash")
        self.assertEqual(pname, "unrouted")
        self.assertEqual(base, "")
        self.assertIsNone(key)

    def test_bare_model_in_routes_resolves(self) -> None:
        cfg = ProxyConfig(
            bind="127.0.0.1",
            port=8787,
            chat_base_url="https://opencode.ai/zen/go/v1",
            api_key_env="OPENCODE_GO_API_KEY",
            timeout_sec=1,
            max_body_bytes=20 * 1024 * 1024,
            providers={"Ollama": {"base_url": "https://ollama.com/v1", "api_key": "k"}},
            routes={"minimax-m2.5": "Ollama"},
        )
        pname, mname, base, key = cfg.resolve_route("minimax-m2.5")
        self.assertEqual(pname, "Ollama")
        self.assertEqual(mname, "minimax-m2.5")


class ProviderConfigTests(unittest.TestCase):
    def test_only_mapped_alias_is_visible_to_codex(self) -> None:
        config = {
            "providers": {
                "local": {
                    "baseUrl": "http://127.0.0.1:11434/v1",
                    "models": ["qwen", "llama"],
                }
            },
            "mappings": {"codex-alias": "local:qwen"},
        }
        with tempfile.NamedTemporaryFile("w", suffix=".json") as file:
            json.dump(config, file)
            file.flush()
            providers, mappings, routes, models = load_provider_config(file.name)

        self.assertIn("local", providers)
        self.assertEqual(mappings["codex-alias"], "local:qwen")
        self.assertEqual(routes["llama"], "local")
        proxy = ProxyConfig(
            bind="127.0.0.1",
            port=8787,
            chat_base_url="",
            api_key_env="OPENCODE_GO_API_KEY",
            timeout_sec=1,
            max_body_bytes=20 * 1024 * 1024,
            providers=providers,
            mappings=mappings,
            routes=routes,
            models=models,
        )
        self.assertEqual(visible_model_ids(proxy), ["codex-alias"])


class CredentialTests(unittest.TestCase):
    def setUp(self) -> None:
        # Reset the module-level cache between tests.
        app_mod._api_key_cache = None

    def test_env_key_wins_without_keychain_lookup(self) -> None:
        with (
            mock.patch.dict(os.environ, {"OPENCODE_GO_API_KEY": "env-key"}, clear=True),
            mock.patch("opencodex_proxy.app.subprocess.run") as run,
        ):
            self.assertEqual(resolve_api_key(make_config(), "req"), "env-key")

        run.assert_not_called()

    def test_keychain_lookup_uses_first_line(self) -> None:
        completed = subprocess.CompletedProcess(
            [
                "security",
                "find-generic-password",
                "-a",
                os.environ.get("USER", ""),
                "-s",
                "opencodex-api-key",
                "-w",
            ],
            0,
            stdout="keychain-key\n",
            stderr="",
        )
        with (
            mock.patch.dict(os.environ, {}, clear=True),
            mock.patch("opencodex_proxy.app.subprocess.run", return_value=completed),
        ):
            self.assertEqual(resolve_api_key(make_config(), "req"), "keychain-key")

    def test_missing_key_names_env_and_keychain(self) -> None:
        completed = subprocess.CompletedProcess(
            [
                "security",
                "find-generic-password",
                "-a",
                os.environ.get("USER", ""),
                "-s",
                "opencodex-api-key",
                "-w",
            ],
            1,
            stdout="",
            stderr="could not be found",
        )
        with (
            mock.patch.dict(os.environ, {}, clear=True),
            mock.patch("opencodex_proxy.app.subprocess.run", return_value=completed),
            self.assertRaises(ProxyError) as ctx,
        ):
            resolve_api_key(make_config(), "req")

        self.assertEqual(ctx.exception.status, HTTPStatus.UNAUTHORIZED)
        self.assertIn("$OPENCODE_GO_API_KEY", ctx.exception.message)
        self.assertIn("keychain", ctx.exception.message)


if __name__ == "__main__":
    unittest.main()


class StreamingProviderRoutingTests(unittest.TestCase):
    def setUp(self) -> None:
        app_mod._api_key_cache = None

    def test_resolve_upstream_target_prefers_provider(self) -> None:
        cfg = ProxyConfig(
            bind="127.0.0.1",
            port=8787,
            chat_base_url="",
            api_key_env="OPENCODE_GO_API_KEY",
            timeout_sec=1,
            max_body_bytes=20 * 1024 * 1024,
        )
        base, key = resolve_upstream_target(
            cfg,
            "req",
            {
                "base_url": "http://127.0.0.1:11434/v1",
                "api_key": "local-key",
                "provider": "ollama",
            },
        )
        self.assertEqual(base, "http://127.0.0.1:11434/v1")
        self.assertEqual(key, "local-key")

    def test_resolve_upstream_target_local_without_key(self) -> None:
        cfg = ProxyConfig(
            bind="127.0.0.1",
            port=8787,
            chat_base_url="",
            api_key_env="OPENCODE_GO_API_KEY",
            timeout_sec=1,
            max_body_bytes=20 * 1024 * 1024,
        )
        base, key = resolve_upstream_target(
            cfg,
            "req",
            {
                "base_url": "http://127.0.0.1:11434/v1",
                "api_key": None,
                "provider": "ollama",
            },
        )
        self.assertEqual(base, "http://127.0.0.1:11434/v1")
        self.assertEqual(key, "not-required")

    def test_resolve_upstream_target_requires_route(self) -> None:
        cfg = ProxyConfig(
            bind="127.0.0.1",
            port=8787,
            chat_base_url="",
            api_key_env="OPENCODE_GO_API_KEY",
            timeout_sec=1,
            max_body_bytes=20 * 1024 * 1024,
        )
        with self.assertRaises(ProxyError) as ctx:
            resolve_upstream_target(cfg, "req", None)
        self.assertEqual(ctx.exception.status, HTTPStatus.BAD_REQUEST)
