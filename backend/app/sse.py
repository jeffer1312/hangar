import asyncio
from app.transcript import TranscriptTailer
from app.state import StateMonitor


async def merged_events(name: str, jsonl: str):
    tailer = TranscriptTailer(jsonl)
    monitor = StateMonitor(name)
    queue: asyncio.Queue = asyncio.Queue()

    async def pump(kind, agen):
        try:
            async for item in agen:
                await queue.put((kind, item.model_dump()))
        except Exception as exc:  # surface, never swallow
            await queue.put(("__error__", exc))

    tasks = [
        asyncio.create_task(pump("message", tailer.follow())),
        asyncio.create_task(pump("state", monitor.stream())),
    ]
    try:
        while True:
            event, data = await queue.get()
            if event == "__error__":
                raise data
            yield {"event": event, "data": data}
    finally:
        for t in tasks:
            t.cancel()
