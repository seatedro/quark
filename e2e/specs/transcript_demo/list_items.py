"""Catches: transcript rows losing their list semantics. The conversation
list must expose list items carrying 1-based position and the full set size,
so a reader can say "item 4999 of 5001" for virtualized rows."""

import re

from quark_e2e import Cua, app_tree, main


def spec(cua: Cua):
    frame = app_tree()
    status = frame.find("label")
    total = int(re.match(r"(\d+) messages", status.name).group(1))
    conversation = frame.find("list", "Conversation")
    assert conversation, frame.dump()
    items = conversation.find_all("list item")
    assert items, f"no list items in the conversation:\n{conversation.dump()}"
    positions = [int(item.attributes.get("posinset", 0)) for item in items]
    sizes = {item.attributes.get("setsize") for item in items}
    assert sizes == {str(total)}, f"setsize {sizes}, expected {total}"
    assert positions == sorted(positions) and len(set(positions)) == len(positions), positions
    assert all(1 <= p <= total for p in positions), positions


main(spec)
