# backend/tests/test_stream_buffer.py
import asyncio

from app.adapters.stream_buffer import StreamBuffer


def test_first_periodic_last_and_idle_tail():
    async def run():
        values = []
        periodic = asyncio.Event()
        async def publish(value):
            values.append(value)
            if value == "abc":
                periodic.set()
        buffer = StreamBuffer(publish, interval=0.01)
        await buffer.append("a")
        assert values == ["a"]
        await buffer.append("b")
        await buffer.append("c")
        assert values == ["a"]
        await asyncio.wait_for(periodic.wait(), 1)
        await buffer.append("d")
        await buffer.flush()
        assert values == ["a", "abc", "abcd"]
        await buffer.flush()
        assert values == ["a", "abc", "abcd"]
        await buffer.discard()
    asyncio.run(run())


def test_discard_cancels_old_generation_before_new_text():
    async def run():
        values = []
        async def publish(value):
            values.append(value)
        buffer = StreamBuffer(publish, interval=60)
        await buffer.append("old")
        await buffer.append(" tail")
        await buffer.discard()
        values.append("")
        await buffer.append("new")
        await asyncio.sleep(0.03)
        assert values == ["old", "", "new"]
        assert buffer.value == "new"
        await buffer.discard()
    asyncio.run(run())


def test_seeded_reset_appends_after_prefix():
    async def run():
        values = []
        async def publish(value):
            values.append(value)
        buffer = StreamBuffer(publish)
        await buffer.reset("prefix")
        await buffer.append(" suffix")
        assert buffer.value == "prefix suffix"
        assert values == ["prefix suffix"]
        buffer.invalidate("again")
        await buffer.append(" suffix")
        assert buffer.value == "again suffix"
        assert values[-1] == "again suffix"
        await buffer.discard()
    asyncio.run(run())


def test_timer_failure_is_reported_without_hot_retry_loop(caplog):
    private_text = "private-conversation-fragment"
    async def run():
        errors = []
        failed = asyncio.Event()
        calls = []
        async def publish(value):
            calls.append(value)
            if value != "first":
                raise RuntimeError(private_text)
        def on_error(error):
            errors.append(type(error).__name__)
            failed.set()
            raise LookupError(private_text)
        buffer = StreamBuffer(publish, on_error=on_error, interval=0.01)
        await buffer.append("first")
        await buffer.append(" next")
        await asyncio.wait_for(failed.wait(), 1)
        await asyncio.sleep(0.03)
        assert errors == ["RuntimeError"]
        assert calls == ["first", "first next"]
        await buffer.discard()
    asyncio.run(run())
    failures = [record for record in caplog.records if record.name == "app.adapters.stream_buffer"]
    assert len(failures) == 2
    assert private_text not in caplog.text
    assert all(record.exc_info is None for record in failures)
    assert all("test_stream_buffer.py:" in record.getMessage() for record in failures)
    assert "RuntimeError" in failures[0].getMessage()
    assert "LookupError" in failures[1].getMessage()


def test_rebind_preserves_pending_prefix_and_publishes_without_new_delta():
    async def run():
        published = []
        emitted = asyncio.Event()

        async def publish(value):
            published.append(value)
            if value == "firsttail":
                emitted.set()

        buffer = StreamBuffer(publish, interval=1e10)
        await buffer.append("first")
        await buffer.append("tail")
        buffer.rebind()
        await asyncio.wait_for(emitted.wait(), 1)
        await buffer.append("next")
        await buffer.flush()
        assert published == ["first", "firsttail", "firsttailnext"]
        await buffer.discard()
    asyncio.run(run())


def test_revision_keeps_delta_written_during_blocked_publication():
    async def run():
        entered = asyncio.Event()
        release = asyncio.Event()
        values = []
        async def publish(value):
            if value == "a":
                entered.set()
                await release.wait()
            values.append(value)
        buffer = StreamBuffer(publish, interval=60)
        first = asyncio.create_task(buffer.append("a"))
        await asyncio.wait_for(entered.wait(), 1)
        second = asyncio.create_task(buffer.append("b"))
        await asyncio.sleep(0)
        assert buffer.value == "ab"
        release.set()
        await asyncio.gather(first, second)
        await buffer.flush()
        assert values == ["a", "ab"]
        await buffer.discard()
    asyncio.run(run())


def test_discard_waits_for_inflight_publication_before_authoritative_clear():
    async def run():
        entered = asyncio.Event()
        release = asyncio.Event()
        values = []
        async def publish(value):
            if value == "old":
                entered.set()
                await release.wait()
            values.append(value)
        buffer = StreamBuffer(publish, interval=60)
        first = asyncio.create_task(buffer.append("old"))
        await asyncio.wait_for(entered.wait(), 1)
        closing = asyncio.create_task(buffer.discard())
        await asyncio.sleep(0)
        assert buffer.value == ""
        assert not closing.done()
        release.set()
        await asyncio.gather(first, closing)
        values.append("")  # o adapter limpa source só DEPOIS de discard
        await buffer.append("new")
        await buffer.flush()
        assert values == ["old", "", "new"]
        await buffer.discard()
    asyncio.run(run())


def test_full_snapshot_keeps_unicode_escapes_and_older_snapshot_alias():
    import json
    async def run():
        values = []
        async def publish(value):
            values.append(value)
        buffer = StreamBuffer(publish, interval=60)
        expected = {"command": 'echo "ação"\nlinha 😀', "path": "C:\\pasta\\arquivo"}
        raw = json.dumps(expected, ensure_ascii=False)
        for char in raw:
            await buffer.append(char)
        first_snapshot = values[0]
        await buffer.flush()
        assert first_snapshot == "{"
        assert values[0] == first_snapshot
        assert values[-1] == raw
        assert json.loads(values[-1]) == expected
        await buffer.discard()
    asyncio.run(run())
