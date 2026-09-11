#!/usr/bin/env python3
"""JSON bridge to Anki's official Python/Rust backend.

The Rust TUI starts this helper for one operation at a time, so the official
collection lock is never held while the terminal is waiting for input.
"""

from __future__ import annotations

import html
import json
import re
import sys
import threading
import time
from datetime import datetime, timedelta, timezone
from html.parser import HTMLParser
from importlib.metadata import version
from pathlib import Path

from anki.collection import Collection
from anki.scheduler.v3 import CardAnswer
from anki.scheduler_pb2 import SchedulingStates
from anki.sync_pb2 import SyncAuth
from anki.template import TemplateRenderContext

PROGRESS_PREFIX = "ANKI_TUI_PROGRESS "


def emit_progress(
    stage: str,
    current: int = 0,
    total: int | None = None,
    detail: str = "",
) -> None:
    message = {
        "stage": stage,
        "current": current,
        "total": total,
        "detail": detail,
    }
    print(
        PROGRESS_PREFIX + json.dumps(message, ensure_ascii=False),
        file=sys.stderr,
        flush=True,
    )


def utc(seconds: float) -> str:
    return datetime.fromtimestamp(seconds, timezone.utc).isoformat().replace("+00:00", "Z")


def card_state(card) -> str:
    return {0: "new", 1: "learning", 2: "review", 3: "relearning"}.get(
        int(card.type), "new"
    )


def card_due(col: Collection, card) -> str:
    if int(card.queue) in (1, 3, 4) and int(card.due) > 1_000_000_000:
        return utc(card.due)
    if int(card.type) in (2, 3):
        return utc(time.time() + (int(card.due) - int(col.sched.today)) * 86400)
    return utc(max(int(card.id) / 1000, 0))


def card_json(col: Collection, card) -> dict:
    memory = card.memory_state
    last_review = card.last_review_time or int(card.id / 1000)
    return {
        "id": int(card.id),
        "note_id": int(card.nid),
        "deck_id": int(card.did),
        "due": card_due(col, card),
        "stability": float(memory.stability) if memory else 0.0,
        "difficulty": float(memory.difficulty) if memory else 0.0,
        "elapsed_days": 0,
        "scheduled_days": int(card.ivl),
        "reps": int(card.reps),
        "lapses": int(card.lapses),
        "state": card_state(card),
        "last_review": utc(last_review),
        "suspended": int(card.queue) == -1,
        "created_at": utc(max(int(card.id) / 1000, 0)),
    }


def note_fields(note) -> list[dict]:
    return [{"name": name, "value": value} for name, value in note.items()]


def inject_audio(text: str, tags: list, side: str) -> str:
    for index, tag in enumerate(tags):
        filename = getattr(tag, "filename", None)
        replacement = f"[sound:{filename}]" if filename else ""
        text = text.replace(f"[anki:play:{side}:{index}]", replacement)
    return re.sub(r"\[anki:play:[^]]+\]", "", text)


def rendered_sides(card) -> tuple[str, str, str]:
    output = card.render_output()
    question = inject_audio(output.question_text, output.question_av_tags, "q")
    answer = inject_audio(output.answer_text, output.answer_av_tags, "a")
    return question, answer, output.css


def preview_card(col: Collection, card_id: int, fields: list[dict], tags: str | None = None) -> dict:
    """Render the card template with draft field values without writing the note."""
    card = col.get_card(card_id)
    note = col.get_note(card.nid)
    expected = [name for name, _ in note.items()]
    if len(fields) != len(note.fields):
        raise ValueError("note field count changed; refusing preview")
    if [field["name"] for field in fields] != expected:
        raise ValueError("note field names changed; refusing preview")
    note.fields = [field["value"] for field in fields]
    if tags is not None:
        note.set_tags_from_str(tags)
    notetype = note.note_type()
    template = notetype["tmpls"][int(card.ord)]
    output = TemplateRenderContext.from_card_layout(
        note, card, notetype, template, False
    ).render()
    question = inject_audio(output.question_text, output.question_av_tags, "q")
    answer = inject_audio(output.answer_text, output.answer_av_tags, "a")
    css = output.css
    return {
        "front": question,
        "back": answer,
        "answer_includes_question": True,
        "front_document": terminal_document(question, css, int(card.ord)),
        "back_document": terminal_document(answer, css, int(card.ord)),
    }


def css_declarations(value: str) -> dict[str, str]:
    declarations = {}
    for declaration in value.split(";"):
        if ":" not in declaration:
            continue
        name, content = declaration.split(":", 1)
        name = name.strip().lower()
        content = re.sub(r"\s*!important\s*$", "", content.strip(), flags=re.I)
        if name and content:
            declarations[name] = content
    return declarations


def css_rules(value: str) -> list[tuple[str, dict[str, str]]]:
    value = re.sub(r"/\*.*?\*/", "", value, flags=re.S)
    rules = []
    for match in re.finditer(r"([^{}]+)\{([^{}]*)\}", value):
        declarations = css_declarations(match.group(2))
        for selector in match.group(1).split(","):
            selector = selector.strip()
            if declarations and selector and not selector.startswith("@"):
                rules.append((selector, declarations))
    return rules


def selector_part_matches(part: str, node: dict) -> bool:
    part = re.sub(r"\[[^]]*\]", "", part)
    part = re.sub(r":{1,2}[\w()-]+", "", part)
    if not part or part == "*":
        return True
    tag = re.match(r"^[a-zA-Z][\w-]*", part)
    if tag and tag.group(0).lower() != node["tag"]:
        return False
    ids = re.findall(r"#([\w-]+)", part)
    if ids and node["id"] not in ids:
        return False
    classes = re.findall(r"\.([\w-]+)", part)
    return all(name in node["classes"] for name in classes)


def selector_matches(selector: str, path: list[dict]) -> bool:
    if ":root" in selector:
        return len(path) == 1
    parts = [part for part in re.split(r"\s+|[>+~]", selector) if part]
    if not parts or not selector_part_matches(parts[-1], path[-1]):
        return False
    ancestor = len(path) - 2
    for part in reversed(parts[:-1]):
        while ancestor >= 0 and not selector_part_matches(part, path[ancestor]):
            ancestor -= 1
        if ancestor < 0:
            return False
        ancestor -= 1
    return True


def apply_declarations(style: dict, declarations: dict[str, str]) -> None:
    if color := declarations.get("color"):
        style["color"] = color
    background = declarations.get("background-color") or declarations.get("background")
    if background and not any(token in background for token in ("url(", "gradient(")):
        style["background"] = background.split()[0]
    if alignment := declarations.get("text-align"):
        style["alignment"] = alignment.lower()
    if weight := declarations.get("font-weight"):
        style["bold"] = weight.lower() in ("bold", "bolder") or (
            weight.isdigit() and int(weight) >= 600
        )
    if font_style := declarations.get("font-style"):
        style["italic"] = font_style.lower() in ("italic", "oblique")
    if decoration := declarations.get("text-decoration"):
        style["underline"] = "underline" in decoration.lower()
        style["crossed_out"] = "line-through" in decoration.lower()


class TerminalCardParser(HTMLParser):
    BLOCK_TAGS = {
        "address",
        "article",
        "aside",
        "blockquote",
        "div",
        "footer",
        "h1",
        "h2",
        "h3",
        "h4",
        "h5",
        "h6",
        "header",
        "li",
        "main",
        "ol",
        "p",
        "pre",
        "section",
        "table",
        "td",
        "tr",
        "ul",
    }
    SOUND_RE = re.compile(r"\[sound:([^]]+)\]", re.I)

    def __init__(self, css: str, card_ord: int):
        super().__init__(convert_charrefs=True)
        self.rules = css_rules(css)
        self.blocks: list[dict] = []
        self.runs: list[dict] = []
        self.nodes: list[dict] = []
        self.skip_depth = 0
        root = {
            "tag": "body",
            "id": "",
            "classes": {"card", f"card{card_ord + 1}"},
        }
        self.root = root
        root_style = self.computed_style({}, root, None)
        self.styles = [root_style]

    def computed_style(self, attrs: dict, node: dict, tag: str | None) -> dict:
        parent = self.styles[-1] if hasattr(self, "styles") and self.styles else {}
        style = dict(parent)
        if tag in ("b", "strong"):
            style["bold"] = True
        if tag in ("i", "em"):
            style["italic"] = True
        if tag == "u":
            style["underline"] = True
        if tag in ("s", "strike", "del"):
            style["crossed_out"] = True
        path = [self.root, *self.nodes]
        if node is not self.root:
            path.append(node)
        for selector, declarations in self.rules:
            if selector_matches(selector, path):
                apply_declarations(style, declarations)
        if attrs.get("align"):
            style["alignment"] = attrs["align"].lower()
        apply_declarations(style, css_declarations(attrs.get("style", "")))
        return style

    @staticmethod
    def node(tag: str, attrs: dict) -> dict:
        return {
            "tag": tag,
            "id": attrs.get("id", ""),
            "classes": set(attrs.get("class", "").split()),
        }

    def handle_starttag(self, tag: str, attrs_list: list[tuple[str, str | None]]) -> None:
        tag = tag.lower()
        if tag in ("script", "style"):
            self.skip_depth += 1
            return
        if self.skip_depth:
            return
        attrs = {name.lower(): value or "" for name, value in attrs_list}
        if tag == "hr":
            self.flush()
            self.blocks.append({"kind": "separator"})
            return
        if tag == "br":
            self.flush(force_spacer=True)
            return
        if tag == "img":
            self.flush()
            if source := attrs.get("src"):
                self.blocks.append({"kind": "image", "source": source})
            return
        if tag in self.BLOCK_TAGS:
            self.flush()
        node = self.node(tag, attrs)
        style = self.computed_style(attrs, node, tag)
        self.nodes.append(node)
        self.styles.append(style)

    def handle_startendtag(
        self, tag: str, attrs_list: list[tuple[str, str | None]]
    ) -> None:
        self.handle_starttag(tag, attrs_list)

    def handle_endtag(self, tag: str) -> None:
        tag = tag.lower()
        if tag in ("script", "style"):
            self.skip_depth = max(0, self.skip_depth - 1)
            return
        if self.skip_depth or tag in ("hr", "br", "img"):
            return
        if tag in self.BLOCK_TAGS:
            self.flush()
        if self.nodes:
            self.nodes.pop()
            self.styles.pop()

    def handle_data(self, data: str) -> None:
        if self.skip_depth:
            return
        cursor = 0
        for match in self.SOUND_RE.finditer(data):
            self.append_text(data[cursor : match.start()])
            self.flush()
            self.append_audio(match.group(1))
            cursor = match.end()
        self.append_text(data[cursor:])

    def append_text(self, value: str) -> None:
        value = re.sub(r"\s+", " ", value)
        if not self.runs:
            value = value.lstrip()
        if not value:
            return
        style = self.styles[-1]
        run = {
            "text": value,
            "color": style.get("color", ""),
            "bold": bool(style.get("bold")),
            "italic": bool(style.get("italic")),
            "underline": bool(style.get("underline")),
            "crossed_out": bool(style.get("crossed_out")),
        }
        if self.runs and all(
            self.runs[-1].get(key) == run.get(key)
            for key in ("color", "bold", "italic", "underline", "crossed_out")
        ):
            self.runs[-1]["text"] += value
        else:
            self.runs.append(run)

    def append_audio(self, source: str) -> None:
        if self.blocks and self.blocks[-1]["kind"] == "audio":
            self.blocks[-1]["sources"].append(source)
        else:
            self.blocks.append({"kind": "audio", "sources": [source]})

    def flush(self, force_spacer: bool = False) -> None:
        if self.runs:
            self.runs[-1]["text"] = self.runs[-1]["text"].rstrip()
            self.runs = [run for run in self.runs if run["text"]]
        if self.runs:
            style = self.styles[-1]
            self.blocks.append(
                {
                    "kind": "text",
                    "runs": self.runs,
                    "alignment": style.get("alignment", "center"),
                    "background": style.get("background", ""),
                }
            )
        elif force_spacer and (
            not self.blocks or self.blocks[-1]["kind"] != "spacer"
        ):
            self.blocks.append({"kind": "spacer"})
        self.runs = []

    def document(self) -> dict:
        self.flush()
        while self.blocks and self.blocks[-1]["kind"] == "spacer":
            self.blocks.pop()
        return {
            "background": self.styles[0].get("background", ""),
            "blocks": self.blocks,
        }


def terminal_document(value: str, css: str, card_ord: int) -> dict:
    parser = TerminalCardParser(css, card_ord)
    parser.feed(value)
    parser.close()
    return parser.document()


def plain_text(value: str) -> str:
    value = re.sub(r"(?is)<style\b[^>]*>.*?</style>", "", value)
    value = re.sub(r"(?i)<(?:br|hr)\s*/?>", "\n", value)
    value = re.sub(r"(?i)</(?:div|p|li|tr|h[1-6])\s*>", "\n", value)
    value = re.sub(r"(?is)<[^>]+>", "", value)
    value = re.sub(r"\[(?:sound|anki:play):[^]]+\]", "", value)
    value = html.unescape(value).replace("\u2068", "").replace("\u2069", "")
    return " ".join(value.split())


def flatten_decks(node, out: list[dict]) -> None:
    for child in node.children:
        out.append(
            {
                "deck_id": int(child.deck_id),
                "name": child.name,
                "new": int(child.new_count),
                "learning": int(child.learn_count),
                "review": int(child.review_count),
                "total": int(child.total_including_children),
                "level": max(int(child.level) - 1, 0),
            }
        )
        flatten_decks(child, out)


def deck_options(col: Collection, deck_id: int) -> tuple[int, int]:
    config = col.decks.config_dict_for_deck_id(deck_id)
    return int(config["new"]["perDay"]), int(config["rev"]["perDay"])


def study_queue(col: Collection, deck_id: int, limit: int) -> list[dict]:
    col.decks.select(deck_id)
    queued = col.sched.get_queued_cards(fetch_limit=limit)
    cards = []
    for item in queued.cards:
        card = col.get_card(item.card.id)
        question, answer, css = rendered_sides(card)
        intervals = [
            label.replace("\u2068", "").replace("\u2069", "")
            for label in col.sched.describe_next_states(item.states)
        ]
        cards.append(
            {
                "card": card_json(col, card),
                "front": question,
                "back": answer,
                "tags": card.note().string_tags().strip(),
                "deck_name": item.context.deck_name,
                "answer_intervals": intervals,
                "answer_includes_question": True,
                "front_document": terminal_document(question, css, int(card.ord)),
                "back_document": terminal_document(answer, css, int(card.ord)),
                "scheduling_states_hex": item.states.SerializeToString().hex(),
                "fields": note_fields(card.note()),
            }
        )
    return cards


def browse(col: Collection, query: str, limit: int) -> list[dict]:
    rows = []
    for card_id in list(col.find_cards(query, order=True))[:limit]:
        card = col.get_card(card_id)
        note = card.note()
        question, answer, _css = rendered_sides(card)
        rows.append(
            {
                "card_id": int(card.id),
                "note_id": int(card.nid),
                "deck_name": col.decks.name(card.did),
                "front": plain_text(question),
                "back": plain_text(answer),
                "tags": note.string_tags().strip(),
                "state": card_state(card),
                "due": card_due(col, card),
                "due_label": due_label(col, card),
                "reps": int(card.reps),
                "lapses": int(card.lapses),
                "suspended": int(card.queue) == -1,
                "fields": note_fields(note),
            }
        )
    return rows


def due_label(col: Collection, card) -> str:
    if int(card.queue) == -1:
        return "Suspended"
    if int(card.type) == 0:
        return f"New #{int(card.due)}"
    if int(card.queue) in (1, 3, 4) and int(card.due) > 1_000_000_000:
        return datetime.fromtimestamp(card.due).strftime("%Y-%m-%d %H:%M")
    day = datetime.now().astimezone().date() + timedelta(
        days=int(card.due) - int(col.sched.today)
    )
    return day.isoformat()


def stats(col: Collection) -> dict:
    day_start_ms = (int(col.sched.day_cutoff) - 86400) * 1000
    now_ms = int(time.time() * 1000)
    scalar = col.db.scalar
    return {
        "total_cards": int(scalar("select count() from cards")),
        "total_notes": int(scalar("select count() from notes")),
        "total_decks": int(col.decks.count()),
        "new_cards": int(scalar("select count() from cards where queue != -1 and type = 0")),
        "learning_cards": int(scalar("select count() from cards where queue != -1 and type = 1")),
        "review_cards": int(scalar("select count() from cards where queue != -1 and type = 2")),
        "relearning_cards": int(scalar("select count() from cards where queue != -1 and type = 3")),
        "suspended_cards": int(scalar("select count() from cards where queue = -1")),
        "reviews_today": int(scalar("select count() from revlog where id >= ?", day_start_ms)),
        "reviews_7d": int(scalar("select count() from revlog where id >= ?", now_ms - 7 * 86400 * 1000)),
        "reviews_30d": int(scalar("select count() from revlog where id >= ?", now_ms - 30 * 86400 * 1000)),
        "mature_cards": int(scalar("select count() from cards where queue != -1 and type = 2 and ivl >= 21")),
    }


def choose_two_field_notetype(col: Collection):
    for notetype in col.models.all():
        names = [field["name"].casefold() for field in notetype["flds"]]
        if len(names) == 2 and names == ["front", "back"]:
            return notetype
    for notetype in col.models.all():
        if len(notetype["flds"]) == 2:
            return notetype
    return col.models.current()


def sync_auth(args: dict) -> SyncAuth:
    auth = SyncAuth(hkey=args["hkey"], io_timeout_secs=120)
    if endpoint := args.get("endpoint"):
        auth.endpoint = endpoint
    return auth


def wait_for_media(col: Collection, report_progress: bool = False) -> dict:
    last = None
    while True:
        status = col.media_sync_status()
        if status.HasField("progress"):
            last = status.progress
            if report_progress:
                detail = " · ".join(
                    value.replace("\u2068", "").replace("\u2069", "")
                    for value in (
                        last.added,
                        last.removed,
                        last.checked,
                    )
                    if value
                )
                emit_progress("Syncing media", detail=detail)
        if not status.active:
            break
        time.sleep(0.1)
    if not last:
        return {"added": "", "removed": "", "checked": ""}
    return {
        "added": last.added,
        "removed": last.removed,
        "checked": last.checked,
    }


def clean_progress_text(value: str) -> str:
    return value.replace("\u2068", "").replace("\u2069", "").strip()


def media_progress_detail(media: dict) -> str:
    return " · ".join(
        clean_progress_text(value)
        for value in (media["added"], media["removed"], media["checked"])
        if value
    )


def media_sync_with_collection(col: Collection, auth: SyncAuth) -> dict:
    emit_progress("Starting media sync")
    col.sync_media(auth)
    media = wait_for_media(col, report_progress=True)
    emit_progress("Syncing media", detail=media_progress_detail(media) or "Complete")
    return media


def normal_sync_with_collection(col: Collection, args: dict) -> dict:
    auth = sync_auth(args)
    emit_progress("Syncing collection")
    stop_progress = threading.Event()

    def report_normal_sync_progress() -> None:
        try:
            progress = col.latest_progress()
        except Exception:
            return
        if not progress.HasField("normal_sync"):
            return
        normal = progress.normal_sync
        stage = clean_progress_text(normal.stage) or "Syncing collection"
        detail = " · ".join(
            text
            for text in (
                clean_progress_text(normal.added),
                clean_progress_text(normal.removed),
            )
            if text
        )
        emit_progress(stage, detail=detail)

    def monitor_normal_sync() -> None:
        while not stop_progress.wait(0.1):
            report_normal_sync_progress()

    monitor = threading.Thread(target=monitor_normal_sync, daemon=True)
    monitor.start()
    try:
        output = col.sync_collection(auth, False)
    finally:
        stop_progress.set()
        monitor.join()
        report_normal_sync_progress()

    required = {
        output.NO_CHANGES: "no_changes",
        output.NORMAL_SYNC: "normal_sync",
        output.FULL_SYNC: "full_sync",
        output.FULL_DOWNLOAD: "full_download",
        output.FULL_UPLOAD: "full_upload",
    }.get(output.required, f"unknown_{output.required}")
    media = None
    if output.required == output.NO_CHANGES:
        if output.new_endpoint:
            auth.endpoint = output.new_endpoint
        media = media_sync_with_collection(col, auth)
    return {
        "required": required,
        "server_message": output.server_message,
        "new_endpoint": output.new_endpoint,
        "media": media,
    }


def run_action(col: Collection, action: str, args: dict):
    if action == "health":
        return {"anki_version": version("anki"), "cards": int(col.card_count())}
    if action == "login":
        endpoint = args.get("endpoint") or None
        auth = col.sync_login(args["username"], args["password"], endpoint)
        return {"hkey": auth.hkey, "endpoint": auth.endpoint or endpoint or ""}
    if action == "add_media":
        return col.media.write_data(args["filename"], bytes.fromhex(args["data_hex"]))
    if action == "deck_tree":
        out: list[dict] = []
        flatten_decks(col.sched.deck_due_tree(), out)
        return out
    if action == "get_deck":
        deck_id = int(args["deck_id"])
        deck = col.decks.get(deck_id, default=False)
        if not deck:
            return None
        new_per_day, rev_per_day = deck_options(col, deck_id)
        return {
            "id": deck_id,
            "name": deck["name"],
            "new_per_day": new_per_day,
            "rev_per_day": rev_per_day,
            "created_at": utc(time.time()),
        }
    if action == "create_deck":
        return int(col.decks.add_normal_deck_with_name(args["name"].strip()).id)
    if action == "rename_deck":
        col.decks.rename(int(args["deck_id"]), args["name"].strip())
        return None
    if action == "delete_deck":
        col.decks.remove([int(args["deck_id"])])
        return None
    if action == "update_deck_options":
        config = col.decks.config_dict_for_deck_id(int(args["deck_id"]))
        config["new"]["perDay"] = int(args["new_per_day"])
        config["rev"]["perDay"] = int(args["rev_per_day"])
        col.decks.update_config(config)
        return None
    if action == "add_note":
        notetype = choose_two_field_notetype(col)
        note = col.new_note(notetype)
        if note.fields:
            note.fields[0] = args["front"].strip()
        if len(note.fields) > 1:
            note.fields[1] = args["back"].strip()
        note.set_tags_from_str(args.get("tags", ""))
        col.add_note(note, int(args["deck_id"]))
        return int(note.id)
    if action == "update_note":
        note = col.get_note(int(args["note_id"]))
        incoming = args["fields"]
        if len(incoming) != len(note.fields):
            raise ValueError("note field count changed; refusing destructive update")
        expected = [name for name, _ in note.items()]
        if [field["name"] for field in incoming] != expected:
            raise ValueError("note field names changed; refusing destructive update")
        note.fields = [field["value"] for field in incoming]
        note.set_tags_from_str(args.get("tags", ""))
        col.update_note(note)
        return None
    if action == "preview_card":
        return preview_card(
            col,
            int(args["card_id"]),
            args["fields"],
            args.get("tags"),
        )
    if action == "delete_card":
        col.remove_notes_by_card([int(args["card_id"])])
        return None
    if action == "set_suspended":
        card_id = int(args["card_id"])
        if args["suspended"]:
            col.sched.suspend_cards([card_id])
        else:
            col.sched.unsuspend_cards([card_id])
        return None
    if action == "forget_card":
        col.sched.schedule_cards_as_new([int(args["card_id"])])
        return None
    if action == "forget_deck":
        deck_id = int(args["deck_id"])
        deck_ids = col.decks.deck_and_child_ids(deck_id)
        placeholders = ",".join("?" for _ in deck_ids)
        ids = col.db.list(
            f"select id from cards where did in ({placeholders}) or odid in ({placeholders})",
            *(list(deck_ids) + list(deck_ids)),
        )
        if ids:
            col.sched.schedule_cards_as_new(ids)
        return len(ids)
    if action == "study_queue":
        return study_queue(col, int(args["deck_id"]), int(args["limit"]))
    if action == "answer_card":
        card = col.get_card(int(args["card_id"]))
        states_hex = args.get("scheduling_states_hex", "")
        states = (
            SchedulingStates.FromString(bytes.fromhex(states_hex))
            if states_hex
            else col._backend.get_scheduling_states(card.id)
        )
        rating = [CardAnswer.AGAIN, CardAnswer.HARD, CardAnswer.GOOD, CardAnswer.EASY][
            int(args["rating"]) - 1
        ]
        card.start_timer()
        answer = col.sched.build_answer(card=card, states=states, rating=rating)
        answer.milliseconds_taken = max(0, int(args.get("milliseconds_taken", 0)))
        col.sched.answer_card(answer)
        return study_queue(col, int(args["deck_id"]), int(args.get("limit", 50)))
    if action == "browse":
        return browse(col, args.get("query", ""), int(args["limit"]))
    if action == "stats":
        return stats(col)
    if action == "normal_sync":
        return normal_sync_with_collection(col, args)
    if action == "media_sync":
        return media_sync_with_collection(col, sync_auth(args))
    raise ValueError(f"unknown action: {action}")


def full_sync_with_collection(col: Collection, action: str, args: dict) -> dict:
    auth = sync_auth(args)

    # The initial sync negotiates the account's shard and media USN. A direct
    # full sync against the generic endpoint does not follow this redirect.
    emit_progress("Negotiating with AnkiWeb")
    output = col.sync_collection(auth, True)
    new_endpoint = output.new_endpoint
    if new_endpoint:
        auth.endpoint = new_endpoint

    transfer_stage = (
        "Uploading collection" if action == "full_upload" else "Downloading collection"
    )
    emit_progress(transfer_stage)
    stop_progress = threading.Event()

    def report_full_sync_progress(final: bool = False) -> None:
        try:
            progress = col.latest_progress()
        except Exception:
            return
        if progress.HasField("full_sync"):
            total = int(progress.full_sync.total)
            if total:
                emit_progress(
                    transfer_stage,
                    current=int(progress.full_sync.transferred),
                    total=total,
                )
                return
        if final:
            size = Path(col.path).stat().st_size
            emit_progress(transfer_stage, current=size, total=size)

    def monitor_full_sync() -> None:
        while not stop_progress.wait(0.1):
            report_full_sync_progress()

    col.close_for_full_sync()
    monitor = threading.Thread(target=monitor_full_sync, daemon=True)
    monitor.start()
    transfer_complete = False
    try:
        col.full_upload_or_download(
            auth=auth,
            server_usn=output.server_media_usn,
            upload=action == "full_upload",
        )
        transfer_complete = True
    finally:
        stop_progress.set()
        monitor.join()
        report_full_sync_progress(final=transfer_complete)
        col.reopen(after_full_sync=True)

    # Access after reopen validates that the result is an Anki collection. The
    # full sync starts media syncing with the negotiated server media USN.
    emit_progress("Validating collection")
    cards = int(col.card_count())
    emit_progress("Syncing media")
    media = wait_for_media(col, report_progress=True)
    return {"cards": cards, "media": media, "new_endpoint": new_endpoint}


def full_sync(path: str, action: str, args: dict):
    col = Collection(path)
    try:
        return full_sync_with_collection(col, action, args)
    finally:
        if col.db:
            col.close()


def main() -> None:
    request = json.load(sys.stdin)
    path = str(Path(request["collection"]).expanduser())
    action = request["action"]
    args = request.get("args", {})
    # Anki's backend may print diagnostics (e.g. "blocked main thread") to
    # stdout. Keep the JSON protocol on a dedicated stream.
    protocol_out = sys.stdout
    sys.stdout = sys.stderr
    try:
        try:
            if action in ("full_download", "full_upload"):
                result = full_sync(path, action, args)
            else:
                col = Collection(path)
                try:
                    result = run_action(col, action, args)
                finally:
                    col.close()
            response = {"ok": True, "result": result}
        except Exception as error:
            response = {"ok": False, "error": f"{type(error).__name__}: {error}"}
    finally:
        sys.stdout = protocol_out
    json.dump(response, protocol_out, ensure_ascii=False)
    protocol_out.write("\n")


if __name__ == "__main__":
    main()
