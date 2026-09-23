"""Contract tests for quote posts (Mastodon 4.5+; next_phase.md §4 P1).

Posting with ``quoted_status_id`` produces a status whose ``quote`` embeds the
quoted status, and the quoted status's ``/quotes`` lists the quoting status.
"""

from __future__ import annotations

from mastodon import Mastodon


def test_quote_post_embeds_quoted_status(alice: Mastodon, bob: Mastodon) -> None:
    original = bob.status_post("the original take")
    quoting = alice.status_post("hot take, see below", quoted_status_id=original.id)

    assert quoting.quote is not None
    assert quoting.quote.state == "accepted"
    assert quoting.quote.quoted_status is not None
    assert quoting.quote.quoted_status.id == original.id
    assert "the original take" in quoting.quote.quoted_status.content

    # Refetching preserves the quote relationship.
    refetched = alice.status(quoting.id)
    assert refetched.quote.quoted_status.id == original.id


def test_non_quote_status_has_null_quote(alice: Mastodon) -> None:
    plain = alice.status_post("nothing quoted here")
    assert plain.quote is None


def test_quotes_endpoint_lists_quoting_statuses(alice: Mastodon, bob: Mastodon) -> None:
    original = bob.status_post("quote bait")
    q1 = alice.status_post("quoting once", quoted_status_id=original.id)
    q2 = bob.status_post("quoting twice", quoted_status_id=original.id)

    quotes = alice.status_quotes(original.id)
    ids = {s.id for s in quotes}
    assert q1.id in ids
    assert q2.id in ids

    # A status nobody quotes has an empty quote list.
    lonely = alice.status_post("nobody quotes me")
    assert not list(alice.status_quotes(lonely.id))


def test_quote_of_missing_status_is_dropped(alice: Mastodon) -> None:
    # An unresolvable quoted_status_id must not 500; the status posts without a quote.
    status = alice.status_post("quoting a ghost", quoted_status_id="0")
    assert status.quote is None
