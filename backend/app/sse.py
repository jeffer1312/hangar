import asyncio
from app.transcript import TranscriptTailer
from app.state import StateMonitor


async def merged_events(name: str, jsonl: str):
    tailer = TranscriptTailer(jsonl)
    monitor = StateMonitor(name)
    queue: asyncio.Queue = asyncio.Queue()

    async def pump_messages():
        async for ev in tailer.follow():
            await queue.put(("message", ev.model_dump()))

    async def pump_state():
        async for st in monitor.stream():
            await queue.put(("state", st.model_dump()))

    tasks = [asyncio.create_task(pump_messages()), asyncio.create_task(pump_state())]
    try:
        while True:
            event, data = await queue.get()
            yield {"event": event, "data": data}
    finally:
        for t in tasks:
            t.cancel()
