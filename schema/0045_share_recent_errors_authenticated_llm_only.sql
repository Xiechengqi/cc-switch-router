-- Recent errors are a user-facing LLM request diagnostic, not a general
-- Router/API error log. Remove historical control-plane, unauthenticated, and
-- non-inference rows; runtime capture applies the same boundary going forward.
DELETE FROM share_request_error_snapshots
 WHERE caller_email IS NULL
    OR method IS NULL
    OR UPPER(method) <> 'POST'
    OR request_source NOT IN ('user', 'dashboard_test')
    OR NOT (
        path LIKE '/gemini/%'
        OR path LIKE '/v1beta/%'
        OR path LIKE '/v1/models/%:generateContent%'
        OR path LIKE '/v1/models/%:streamGenerateContent%'
        OR path LIKE '/anthropic/%'
        OR path LIKE '/claude/%'
        OR path LIKE '/v1/messages%'
        OR path LIKE '/codex/%'
        OR path LIKE '/openai/%'
        OR path LIKE '/backend-api/codex/%'
        OR path LIKE '/v1/chat/%'
        OR path LIKE '/v1/v1/chat/%'
        OR path LIKE '/v1/completions%'
        OR path LIKE '/v1/v1/completions%'
        OR path LIKE '/v1/responses%'
        OR path LIKE '/v1/v1/responses%'
        OR path LIKE '/v1/images/generations%'
        OR path LIKE '/images/generations%'
        OR path = '/responses'
        OR path LIKE '/responses/%'
        OR path LIKE '/chat/%'
    );
