#!/usr/bin/env python3

import unittest
from types import SimpleNamespace
from unittest.mock import patch

from scripts import anki_backend


class CardRenderingTests(unittest.TestCase):
    def test_preserves_template_structure_styles_and_audio_group(self):
        html = """
        <div style="color:#FF80DD">avoid</div>
        <hr><img src="avoid.jpg"><hr>
        <div class="meaning">Meaning: To <i>avoid</i> something.</div>
        <hr>[sound:word.mp3] [sound:meaning.mp3] [sound:example.mp3]
        """
        css = """
        .card { text-align:center; color:white; background-color:black; }
        .meaning { color:#00aaaa; text-align:left; }
        """

        document = anki_backend.terminal_document(html, css, 0)

        self.assertEqual(document["background"], "black")
        self.assertEqual(
            [block["kind"] for block in document["blocks"]],
            ["text", "separator", "image", "separator", "text", "separator", "audio"],
        )
        meaning = document["blocks"][4]
        self.assertEqual(meaning["alignment"], "left")
        self.assertEqual(meaning["runs"][1]["text"], "avoid")
        self.assertTrue(meaning["runs"][1]["italic"])
        self.assertEqual(meaning["runs"][1]["color"], "#00aaaa")
        self.assertEqual(
            document["blocks"][6]["sources"],
            ["word.mp3", "meaning.mp3", "example.mp3"],
        )

    def test_applies_card_ordinal_and_id_css(self):
        css = ".card { color:white } .card2 { color:red } #rubric { background:#1d6695 }"
        first = anki_backend.terminal_document('<div id="rubric">Title</div>', css, 0)
        second = anki_backend.terminal_document('<div id="rubric">Title</div>', css, 1)

        self.assertEqual(first["blocks"][0]["runs"][0]["color"], "white")
        self.assertEqual(second["blocks"][0]["runs"][0]["color"], "red")
        self.assertEqual(second["blocks"][0]["background"], "#1d6695")


class FullSyncTests(unittest.TestCase):
    def test_negotiates_endpoint_and_media_usn_before_download(self):
        events = []
        progress_events = []
        auth = SimpleNamespace(endpoint="")
        output = SimpleNamespace(
            new_endpoint="https://sync.example.test/",
            server_media_usn=42,
        )

        class Collection:
            db = object()
            path = __file__

            def sync_collection(self, received_auth, sync_media):
                events.append(("sync_collection", received_auth.endpoint, sync_media))
                return output

            def close_for_full_sync(self):
                events.append(("close_for_full_sync",))
                self.db = None

            def full_upload_or_download(self, *, auth, server_usn, upload):
                events.append(
                    (
                        "full_upload_or_download",
                        auth.endpoint,
                        server_usn,
                        upload,
                    )
                )

            def latest_progress(self):
                return SimpleNamespace(
                    HasField=lambda name: name == "full_sync",
                    full_sync=SimpleNamespace(transferred=1024, total=1024),
                )

            def reopen(self, *, after_full_sync):
                events.append(("reopen", after_full_sync))
                self.db = object()

            def card_count(self):
                return 7

            def media_sync_status(self):
                events.append(("media_sync_status",))
                return SimpleNamespace(active=False, HasField=lambda _name: False)

        with (
            patch.object(anki_backend, "sync_auth", return_value=auth),
            patch.object(
                anki_backend,
                "emit_progress",
                side_effect=lambda stage, **_kwargs: progress_events.append(stage),
            ),
        ):
            result = anki_backend.full_sync_with_collection(
                Collection(), "full_download", {}
            )

        self.assertEqual(result["cards"], 7)
        self.assertEqual(result["new_endpoint"], output.new_endpoint)
        self.assertEqual(
            progress_events,
            [
                "Negotiating with AnkiWeb",
                "Downloading collection",
                "Downloading collection",
                "Validating collection",
                "Syncing media",
            ],
        )
        self.assertEqual(
            events,
            [
                ("sync_collection", "", True),
                ("close_for_full_sync",),
                (
                    "full_upload_or_download",
                    output.new_endpoint,
                    output.server_media_usn,
                    False,
                ),
                ("reopen", True),
                ("media_sync_status",),
            ],
        )

    def test_normal_sync_reports_collection_and_media_progress(self):
        progress_events = []
        sync_output = SimpleNamespace(
            required=0,
            NO_CHANGES=0,
            NORMAL_SYNC=1,
            FULL_SYNC=2,
            FULL_DOWNLOAD=3,
            FULL_UPLOAD=4,
            server_message="",
            new_endpoint="https://sync.example.test/",
        )

        class Collection:
            def sync_collection(self, auth, sync_media):
                self.auth = auth
                self.sync_media_during_collection = sync_media
                return sync_output

            def latest_progress(self):
                return SimpleNamespace(
                    HasField=lambda name: name == "normal_sync",
                    normal_sync=SimpleNamespace(
                        stage="Uploading changes", added="Added 2", removed="Removed 1"
                    ),
                )

            def sync_media(self, auth):
                self.media_auth = auth

            def media_sync_status(self):
                return SimpleNamespace(active=False, HasField=lambda _name: False)

        collection = Collection()
        with (
            patch.object(
                anki_backend,
                "sync_auth",
                return_value=SimpleNamespace(endpoint=""),
            ),
            patch.object(
                anki_backend,
                "emit_progress",
                side_effect=lambda stage, **kwargs: progress_events.append(
                    (stage, kwargs.get("detail", ""))
                ),
            ),
        ):
            result = anki_backend.normal_sync_with_collection(collection, {})

        self.assertEqual(result["required"], "no_changes")
        self.assertFalse(collection.sync_media_during_collection)
        self.assertEqual(collection.media_auth.endpoint, sync_output.new_endpoint)
        self.assertIn(("Syncing collection", ""), progress_events)
        self.assertIn(
            ("Uploading changes", "Added 2 · Removed 1"), progress_events
        )
        self.assertIn(("Starting media sync", ""), progress_events)
        self.assertIn(("Syncing media", "Complete"), progress_events)


if __name__ == "__main__":
    unittest.main()
