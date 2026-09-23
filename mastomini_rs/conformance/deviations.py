"""Where mastomini knowingly differs from mastodon_mock, as data.

Keys are ``file::test`` (the upstream test id without the directory).

* ``NOT_APPLICABLE``: mastomini deliberately lacks the feature (federation,
  media, the mock's own helper endpoints, ...). Skipped.
* ``NOT_YET``: planned but not built. Marked xfail with ``strict=True``, so
  the moment one starts passing the suite fails and the entry must go.
* ``DIFFERENT``: mastomini behaves differently on purpose (a household
  server with 140-character posts, not a public instance). Also strict xfail.

Anything failing that is not listed here is a bug, in mastomini or in this
list.
"""

from __future__ import annotations

import pytest

NOT_APPLICABLE: dict[str, str] = {
    'test_contract_admin.py::test_admin_accounts_v2_origin_remote': 'no remote accounts (no federation)',
    'test_contract_admin_extra.py::test_admin_accounts_filtering': "needs the mock's remote account dave@remote.example",
    'test_contract_gaps.py::test_remote_mention_resolves_against_local_row': 'no remote accounts (no federation)',
    'test_contract_discovery.py::test_instance_peers_lists_remote_domains': 'no federation, no peers',
    'test_contract_discovery.py::test_instance_activity_shape': 'no public instance statistics',
    'test_contract_admin.py::test_admin_domain_block_crud': 'no domains to block (no federation)',
    'test_contract_admin.py::test_admin_domain_allow_crud': 'no domains to allow (no federation)',
    'test_contract_admin.py::test_admin_email_domain_block_crud': 'no sign-ups or e-mail addresses',
    'test_contract_admin.py::test_admin_canonical_email_block_crud': 'no sign-ups or e-mail addresses',
    'test_contract_admin.py::test_admin_ip_block_crud': 'no sign-ups; IPs are not recorded',
    'test_contract_discovery.py::test_instance_domain_blocks_reflect_admin_blocks': 'no domain blocks (no federation)',
    'test_contract_gaps.py::test_domain_block_reflected_in_relationship': 'no domain blocks (no federation)',
    'test_last_mile_phase_1_2.py::test_domain_block_can_reject_reports': 'no domain blocks (no federation)',
    'test_last_mile_phase_1_2.py::test_signup_email_block_is_enforced': 'no sign-ups',
    'test_contract_oauth.py::test_self_service_account_creation': 'registrations are closed; the household admin adds members',
    'test_contract_oauth.py::test_duplicate_username_signup_is_rejected': 'registrations are closed',
    'test_contract_oauth.py::test_signup_requires_agreement': 'registrations are closed',
    'test_contract_oauth.py::test_authorization_code_grant_issues_user_token': "uses the mock's password-less account picker",
    'test_contract_oauth.py::test_authorization_code_grant_rejects_disabled_account': 'uses /api/v1/_mock/dev_user',
    'test_contract_oauth.py::test_mock_dev_user_creates_usable_token': 'mock-only helper endpoint',
    'test_contract_oauth.py::test_mock_dev_user_admin_role_and_explicit_username': 'mock-only helper endpoint',
    'test_contract_oauth.py::test_mock_dev_users_lists_tokened_accounts': 'mock-only helper endpoint',
    'test_contract_oauth.py::test_verify_credentials_reports_role_for_staff': 'uses /api/v1/_mock/dev_user',
    'test_contract_admin.py::test_admin_trending_tags_derived_from_local_usage': 'no trends (spec/02: no feed algorithms)',
    'test_contract_admin.py::test_admin_trending_statuses_ranked_by_favourites': 'no trends',
    'test_contract_admin.py::test_admin_trending_links_are_empty': 'no admin trends endpoints',
    'test_contract_discovery.py::test_trending_tags_from_local_hashtags': 'no trends',
    'test_contract_discovery.py::test_trending_statuses_ranks_by_favourites': 'no trends',
    'test_last_mile_phase_1_2.py::test_rejecting_trend_removes_it_from_public_results': 'no trends',
    'test_contract_admin.py::test_admin_measures_shape': 'no admin analytics',
    'test_contract_admin.py::test_admin_dimensions_shape': 'no admin analytics',
    'test_contract_admin.py::test_admin_retention_shape': 'no admin analytics',
    'test_contract_admin_extra.py::test_admin_announcement_create_publish_delete_cycle': 'no announcements (spec/04: later)',
    'test_contract_admin_extra.py::test_admin_announcement_create_requires_text': 'no announcements',
    'test_last_mile_phase_1_2.py::test_scheduled_announcement_visibility': 'no announcements',
    'test_contract_extended.py::test_media_upload_and_attach': 'no post media (spec/01)',
    'test_contract_gaps.py::test_only_media_scopes_to_target_account': 'no post media',
    'test_contract_media.py::test_media_get_round_trips': 'no post media',
    'test_contract_media.py::test_media_update_description_and_focus': 'no post media',
    'test_contract_openapi_backlog.py::test_media_delete': 'no post media',
    'test_contract_extended.py::test_poll_create_and_vote': 'polls are a later phase (spec/04 Tier 3)',
    'test_contract_extended.py::test_scheduled_status': 'no scheduled posts (spec/04 Tier 3)',
    'test_contract_scheduled.py::test_far_future_schedule_returns_scheduled_status': 'no scheduled posts',
    'test_contract_scheduled.py::test_near_term_schedule_publishes_immediately': 'no scheduled posts',
    'test_contract_scheduled.py::test_due_scheduled_status_is_published_on_list': 'no scheduled posts',
    'test_contract_scheduled.py::test_scheduled_status_cannot_be_read_by_another_account': 'no scheduled posts',
    'test_contract_scheduled.py::test_scheduled_status_cannot_be_rescheduled_by_another_account': 'no scheduled posts',
    'test_contract_scheduled.py::test_scheduled_status_cannot_be_deleted_by_another_account': 'no scheduled posts',
    'test_contract_quotes.py::test_quote_post_embeds_quoted_status': 'no quote posts (spec/04 Tier 3)',
    'test_contract_quotes.py::test_quotes_endpoint_lists_quoting_statuses': 'no quote posts',
    'test_contract_tags_quotes.py::test_quote_revoke_hides_quoted_status': 'no quote posts',
    'test_contract_tags_quotes.py::test_update_quote_approval_policy': 'no quote posts',
    'test_contract_tags_quotes.py::test_quote_policy_forced_to_nobody_for_private': 'no quote posts',
    'test_last_mile_phase_1_2.py::test_quote_policy_is_enforced': 'no quote posts',
    'test_contract_extended.py::test_status_translate_pig_latin': 'no translation',
    'test_contract_discovery.py::test_translation_languages_map': 'no translation',
    'test_contract_extended.py::test_status_card_dummy_on_link': 'no preview cards (the board fetches nothing off-box)',
    'test_contract_discovery.py::test_custom_emojis_nonempty_and_shaped': 'no custom emoji (spec/03: 0)',
    'test_contract_openapi_backlog.py::test_push_subscription_round_trip': 'no push (spec/04 Tier 3)',
    'test_contract_openapi_backlog.py::test_oembed_returns_minimal_shape': 'no oEmbed: posts have no public pages',
    'test_contract_discovery.py::test_endorsements_reflect_pins': 'no endorsements (spec/04 Tier 3)',
    'test_contract_openapi_backlog.py::test_account_endorsements_by_id': 'no endorsements',
    'test_contract_discovery.py::test_featured_tags_own_and_by_account': 'no featured tags (max_featured_tags: 0)',
    'test_contract_tags_quotes.py::test_featured_tag_create_and_list': 'no featured tags',
    'test_contract_tags_quotes.py::test_featured_tag_delete_by_id': 'no featured tags',
    'test_contract_tags_quotes.py::test_tag_feature_unfeature_aliases': 'no featured tags',
    'test_contract_tags_quotes.py::test_featured_tags_visible_to_other_accounts': 'no featured tags',
    'test_contract_tags_quotes.py::test_featured_tag_suggestions_exclude_featured': 'no featured tags',
    'test_contract_tags_quotes.py::test_featured_tag_create_is_idempotent': 'no featured tags',
}

NOT_YET: dict[str, str] = {
    'test_contract_core.py::test_locked_account_follow_request': 'follow requests for locked accounts (sprints.md known gap)',
    'test_contract_extended.py::test_status_edit_and_history': 'status edits (Tier 2)',
    'test_contract_extended.py::test_lists': 'lists (Tier 2)',
    'test_contract_lists.py::test_list_crud_lifecycle': 'lists (Tier 2)',
    'test_contract_lists.py::test_list_membership_add_remove': 'lists (Tier 2)',
    'test_contract_lists.py::test_list_owned_by_other_user_is_404': 'lists (Tier 2)',
    'test_contract_timelines.py::test_list_timeline': 'lists (Tier 2)',
    'test_contract_extended.py::test_filters_v2': 'filters (Tier 2)',
    'test_contract_filters.py::test_filter_v1_crud': 'filters (Tier 2)',
    'test_contract_filters.py::test_filter_v2_crud_with_keywords': 'filters (Tier 2)',
    'test_contract_filters.py::test_filter_not_owned_is_404': 'filters (Tier 2)',
    'test_contract_filters.py::test_filter_statuses_v2_empty_on_fresh_filter': 'filters (Tier 2)',
    'test_contract_filters.py::test_filter_statuses_v2_crud': 'filters (Tier 2)',
    'test_contract_filters.py::test_filter_status_not_owned_is_404': 'filters (Tier 2)',
    'test_contract_openapi_backlog.py::test_filter_keyword_v2_get_and_put': 'filters (Tier 2)',
    'test_last_mile_phase_1_2.py::test_warn_and_hide_filters_populate_filtered_results': 'filters (Tier 2)',
    'test_contract_extended.py::test_conversations': 'conversations (Tier 2)',
    'test_contract_openapi_backlog.py::test_conversation_delete': 'conversations (Tier 2)',
    'test_contract_tags_quotes.py::test_tag_follow_unfollow_roundtrip': 'followed hashtags (Tier 2)',
    'test_contract_tags_quotes.py::test_tag_follow_is_idempotent_and_per_account': 'followed hashtags (Tier 2)',
    'test_contract_tags_quotes.py::test_tag_name_normalized': 'followed hashtags (Tier 2)',
    'test_contract_discovery.py::test_followed_tags_reflect_tag_follows': 'followed hashtags (Tier 2)',
    'test_last_mile_phase_1_2.py::test_suggestion_dismissal_persists': 'dismissed suggestions are not remembered',
    'test_last_mile_phase_1_2.py::test_notification_policy_filters_accepts_and_overrides': 'the notification policy is always accept-all',
    'test_last_mile_phase_1_2.py::test_notification_policy_drop_creates_neither_notification_nor_request': 'the notification policy is always accept-all',
}

DIFFERENT: dict[str, str] = {
    'test_contract_core.py::test_instance_info': 'posts are 140 characters, not 500 (spec/03)',
    'test_integration_readonly.py::test_instance_info_shape': 'posts are 140 characters, not 500',
    'test_contract_status_validation.py::test_status_at_the_limit_is_accepted': 'posts are 140 characters, not 500',
    'test_contract_core.py::test_follow_generates_notification': 'the seed already has alice following bob; as on Mastodon, following again is a no-op and does not notify twice',
    'test_contract_accounts_extra.py::test_account_relationships_invalid_id': 'as on Mastodon, relationships skip unknown ids (the mock invents one)',
    'test_bughunt_bulk_by_id.py::test_familiar_followers_bulk': 'as on Mastodon, familiar followers are only accounts the viewer follows',
    'test_contract_admin.py::test_admin_account_delete': 'as on Mastodon (AccountPolicy#destroy?), an account must be suspended before its data is deleted',
    'test_contract_admin.py::test_admin_account_delete_removes_their_statuses': 'an account must be suspended before deletion, as on Mastodon',
    'test_last_mile_phase_1_2.py::test_admin_suspend_blocks_login_and_hides_existing_status': 'reads the public timeline without a token; mastomini requires sign-in (open question #6)',
    'test_last_mile_phase_1_2.py::test_admin_sensitive_and_silence_have_public_effects': "as on Mastodon, authors see their own posts' sensitivity; also reads the public timeline without a token",
    'test_contract_extended.py::test_update_credentials': 'as on Mastodon, the returned note is HTML (plain text is in source.note)',
    'test_contract_gaps.py::test_update_credentials_fields_and_avatar': 'avatars are uploaded in the household app, not update_credentials (sprints.md)',
    'test_bughunt_grouped_notifications.py::test_follow_notifications_group_into_one': 'notifications are never grouped: each is its own group (spec/04)',
    'test_bughunt_grouped_notifications.py::test_reblogs_of_same_status_group': 'notifications are never grouped',
    'test_contract_grouped_notifications.py::test_favourites_of_same_status_group_together': 'notifications are never grouped',
    'test_contract_grouped_notifications.py::test_single_group_fetch_and_accounts': 'notifications are never grouped, so a group has one account',
    'test_contract_grouped_notifications.py::test_grouped_unread_count_counts_groups': 'notifications are never grouped, so groups = notifications',
    'test_contract_oauth.py::test_oauth_userinfo': 'OIDC userinfo is Mastodon 4.4; mastomini advertises 4.3',
    'test_contract_openapi_backlog.py::test_profile_get_and_patch_mirrors_update_credentials': '/api/v1/profile is Mastodon 4.5; mastomini advertises 4.3',
    'test_contract_openapi_backlog.py::test_timelines_direct': '/api/v1/timelines/direct was removed in Mastodon 3.0; use conversations',
}


def _key(item: pytest.Item) -> str:
    path = item.nodeid.split("::", 1)
    return f"{path[0].rsplit('/', 1)[-1]}::{path[1]}" if len(path) == 2 else item.nodeid


def apply(items: list[pytest.Item]) -> None:
    listed = {**NOT_APPLICABLE, **NOT_YET, **DIFFERENT}
    for item in items:
        key = _key(item)
        if key in NOT_APPLICABLE:
            item.add_marker(pytest.mark.skip(reason=f"not applicable: {NOT_APPLICABLE[key]}"))
        elif key in NOT_YET:
            item.add_marker(pytest.mark.xfail(reason=f"not yet: {NOT_YET[key]}", strict=True))
        elif key in DIFFERENT:
            item.add_marker(pytest.mark.xfail(reason=f"deviation: {DIFFERENT[key]}", strict=True))
    stale = set(listed) - {_key(i) for i in items}
    if stale:
        raise pytest.UsageError(f"deviations.py lists tests that no longer exist: {sorted(stale)}")
