-- Fresh-install baseline for the Router business database.
-- Historical SQLite schemas are intentionally unsupported.
CREATE TABLE public_hosts (
            label TEXT PRIMARY KEY COLLATE NOCASE,
            route_id TEXT NOT NULL,
            kind TEXT NOT NULL CHECK(kind IN ('client', 'share', 'market')),
            subject_id TEXT NOT NULL,
            installation_id TEXT,
            target_lane_id TEXT NOT NULL,
            lifecycle TEXT NOT NULL CHECK(lifecycle IN ('active', 'disabled', 'tombstoned')),
            revision INTEGER NOT NULL CHECK(revision > 0),
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );
CREATE UNIQUE INDEX idx_public_hosts_live_subject
            ON public_hosts(kind, subject_id)
            WHERE lifecycle != 'tombstoned';
CREATE UNIQUE INDEX idx_public_hosts_live_route
            ON public_hosts(route_id)
            WHERE lifecycle != 'tombstoned';
CREATE INDEX idx_public_hosts_target_lane
            ON public_hosts(target_lane_id, lifecycle);
CREATE TABLE market_supplier_access_policies (
        supplier_user_id TEXT NOT NULL,
        supplier_email TEXT NOT NULL,
        product_kind TEXT NOT NULL CHECK (product_kind IN ('share', 'client_host')),
        pricing_kind TEXT NOT NULL CHECK (pricing_kind IN ('free', 'paid')),
        mode TEXT NOT NULL CHECK (mode IN ('whitelist', 'blacklist')),
        revision INTEGER NOT NULL DEFAULT 1,
        risk_acknowledged_at TEXT,
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL,
        PRIMARY KEY (supplier_user_id, product_kind, pricing_kind)
    );
CREATE TABLE market_counterparty_access_rules (
        counterparty_id TEXT NOT NULL,
        product_kind TEXT NOT NULL CHECK (product_kind IN ('share', 'client_host')),
        pricing_kind TEXT NOT NULL CHECK (pricing_kind IN ('free', 'paid')),
        decision TEXT NOT NULL CHECK (decision IN ('inherit', 'allow', 'deny')),
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL,
        PRIMARY KEY (counterparty_id, product_kind, pricing_kind)
    );
CREATE TABLE market_counterparties (
            id TEXT PRIMARY KEY,
            supplier_user_id TEXT NOT NULL,
            supplier_email TEXT NOT NULL,
            buyer_user_id TEXT,
            buyer_email TEXT NOT NULL,
            status TEXT NOT NULL CHECK (status IN ('active', 'revoked')),
            revision INTEGER NOT NULL DEFAULT 1,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            revoked_at TEXT,
            UNIQUE (supplier_user_id, buyer_email)
        );
CREATE UNIQUE INDEX uq_market_counterparty_bound_user
            ON market_counterparties(supplier_user_id, buyer_user_id)
            WHERE buyer_user_id IS NOT NULL;
CREATE TABLE market_credit_grants (
            counterparty_id TEXT NOT NULL,
            currency TEXT NOT NULL,
            kind TEXT NOT NULL CHECK (kind IN ('none', 'limited', 'unlimited')),
            limit_minor INTEGER,
            revision INTEGER NOT NULL DEFAULT 1,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            PRIMARY KEY (counterparty_id, currency)
        );
CREATE TABLE market_public_credit_policies (
            supplier_user_id TEXT NOT NULL,
            supplier_email TEXT NOT NULL,
            currency TEXT NOT NULL,
            enabled INTEGER NOT NULL DEFAULT 0,
            limit_minor INTEGER,
            revision INTEGER NOT NULL DEFAULT 1,
            risk_acknowledged_at TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            PRIMARY KEY (supplier_user_id, currency)
        );
CREATE TABLE market_access_events (
            id TEXT PRIMARY KEY,
            supplier_user_id TEXT NOT NULL,
            counterparty_id TEXT,
            actor_user_id TEXT NOT NULL,
            event_type TEXT NOT NULL,
            detail_json TEXT NOT NULL,
            created_at TEXT NOT NULL
        );
CREATE INDEX idx_market_counterparties_supplier_status
            ON market_counterparties(supplier_user_id, status, updated_at);
CREATE INDEX idx_market_access_events_supplier
            ON market_access_events(supplier_user_id, created_at);
CREATE TABLE market_access_requests (
            id TEXT PRIMARY KEY,
            supplier_user_id TEXT NOT NULL,
            supplier_email TEXT NOT NULL,
            buyer_user_id TEXT NOT NULL,
            buyer_email TEXT NOT NULL,
            product_kind TEXT NOT NULL CHECK (product_kind IN ('share', 'client_host')),
            pricing_kind TEXT NOT NULL CHECK (pricing_kind IN ('free', 'paid')),
            target_kind TEXT NOT NULL CHECK (target_kind IN ('share_seat', 'client_host')),
            target_id TEXT NOT NULL,
            target_label TEXT NOT NULL,
            daily_rate_minor INTEGER,
            currency TEXT,
            status TEXT NOT NULL CHECK (status IN ('requested', 'approved', 'rejected', 'cancelled')),
            revision INTEGER NOT NULL DEFAULT 1,
            requested_at TEXT NOT NULL,
            resolved_at TEXT,
            resolved_by_user_id TEXT,
            resolution_reason TEXT,
            resolution_note TEXT
        );
CREATE UNIQUE INDEX uq_market_access_request_requested
            ON market_access_requests(
                supplier_user_id, buyer_user_id, product_kind, pricing_kind
            ) WHERE status = 'requested';
CREATE INDEX idx_market_access_requests_supplier
            ON market_access_requests(supplier_user_id, status, requested_at DESC);
CREATE INDEX idx_market_access_requests_buyer
            ON market_access_requests(buyer_user_id, status, requested_at DESC);
CREATE TABLE router_ssh_hosts (
            id TEXT PRIMARY KEY,
            ip TEXT NOT NULL,
            port INTEGER NOT NULL,
            host_owner_email TEXT NOT NULL,
            country_code TEXT,
            hostname TEXT,
            ssh_host_key_fingerprint TEXT,
            status TEXT NOT NULL,
            installation_id TEXT,
            last_verified_at TEXT,
            last_error TEXT,
            note TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL, ip_intel_json TEXT, provider_id TEXT, daily_rate_minor INTEGER, currency TEXT, free_duration_days INTEGER, offer_revision INTEGER NOT NULL DEFAULT 1,
            UNIQUE(ip, port)
        );
CREATE INDEX idx_router_ssh_hosts_supply
            ON router_ssh_hosts(host_owner_email, status, country_code);
CREATE TABLE provisioning_jobs (
            id TEXT PRIMARY KEY,
            type TEXT NOT NULL,
            host_id TEXT,
            host_owner_email TEXT,
            client_owner_email TEXT,
            selection_owners_json TEXT,
            selection_regions_json TEXT,
            subdomain TEXT,
            installation_id TEXT,
            status TEXT NOT NULL,
            phase TEXT NOT NULL DEFAULT 'pending',
            log_blob TEXT NOT NULL DEFAULT '',
            secret_ref TEXT,
            failure_code TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        , batch_id TEXT, quote_id TEXT, client_owner_user_id TEXT, cleanup_reason TEXT);
CREATE INDEX idx_provisioning_jobs_client
            ON provisioning_jobs(client_owner_email, status, updated_at DESC);
CREATE INDEX idx_provisioning_jobs_host
            ON provisioning_jobs(host_id, status);
CREATE TABLE subdomain_reservations (
            subdomain TEXT PRIMARY KEY COLLATE NOCASE,
            job_id TEXT NOT NULL,
            host_id TEXT,
            client_owner_email TEXT,
            installation_id TEXT,
            expires_at_ms INTEGER NOT NULL
        );
CREATE TABLE client_market_host_import_jobs (
            id TEXT PRIMARY KEY,
            provider_id TEXT NOT NULL,
            owner_email TEXT NOT NULL,
            source_ip TEXT NOT NULL,
            status TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            completed_at TEXT
        );
CREATE TABLE client_market_host_import_items (
            id TEXT PRIMARY KEY,
            job_id TEXT NOT NULL,
            position INTEGER NOT NULL,
            ip TEXT NOT NULL,
            port INTEGER NOT NULL,
            payload_json TEXT NOT NULL,
            status TEXT NOT NULL,
            host_id TEXT,
            error_message TEXT,
            attempts INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            UNIQUE(job_id, position),
            UNIQUE(job_id, ip, port)
        );
CREATE UNIQUE INDEX idx_router_ssh_hosts_installation
            ON router_ssh_hosts(installation_id)
            WHERE installation_id IS NOT NULL;
CREATE UNIQUE INDEX idx_provisioning_jobs_active_host
            ON provisioning_jobs(host_id)
            WHERE host_id IS NOT NULL AND status IN ('pending', 'running');
CREATE INDEX idx_subdomain_reservations_job
            ON subdomain_reservations(job_id);
CREATE UNIQUE INDEX idx_subdomain_reservations_installation
            ON subdomain_reservations(installation_id)
            WHERE installation_id IS NOT NULL;
CREATE INDEX idx_client_market_host_import_jobs_status
            ON client_market_host_import_jobs(status, updated_at);
CREATE INDEX idx_client_market_host_import_items_pending
            ON client_market_host_import_items(job_id, status, position);
CREATE TABLE host_provider_profiles (
            provider_id TEXT PRIMARY KEY,
            owner_email TEXT NOT NULL UNIQUE,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );
CREATE TABLE host_provider_daily_stats (
            provider_id TEXT NOT NULL,
            stat_date TEXT NOT NULL,
            host_total INTEGER NOT NULL,
            idle_total INTEGER NOT NULL,
            allocated_total INTEGER NOT NULL,
            external_client_total INTEGER NOT NULL,
            online_samples INTEGER NOT NULL,
            observed_samples INTEGER NOT NULL,
            anomalous_host_samples INTEGER NOT NULL,
            host_samples INTEGER NOT NULL,
            updated_at TEXT NOT NULL,
            PRIMARY KEY (provider_id, stat_date)
        );
CREATE TABLE account_payment_profiles (
            user_id TEXT PRIMARY KEY,
            owner_email TEXT NOT NULL,
            methods_json TEXT NOT NULL DEFAULT '[]',
            updated_at TEXT NOT NULL
        , contacts_json TEXT NOT NULL DEFAULT '[]');
CREATE TABLE account_payment_methods (
            id TEXT PRIMARY KEY,
            profile_user_id TEXT NOT NULL,
            position INTEGER NOT NULL,
            kind TEXT NOT NULL,
            method_json TEXT NOT NULL,
            enabled INTEGER NOT NULL DEFAULT 1,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            UNIQUE(profile_user_id, position)
        );
CREATE TABLE account_payment_assets (
            id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL,
            source_url TEXT NOT NULL,
            media_type TEXT NOT NULL,
            content BLOB NOT NULL,
            sha256 TEXT NOT NULL,
            created_at TEXT NOT NULL,
            UNIQUE(user_id, source_url, sha256)
        );
CREATE TABLE client_market_allocation_quotes (
            id TEXT PRIMARY KEY,
            client_user_id TEXT NOT NULL,
            client_owner_email TEXT NOT NULL,
            status TEXT NOT NULL,
            fixed_host_id TEXT,
            expires_at TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );
CREATE TABLE client_market_allocation_quote_items (
            id TEXT PRIMARY KEY,
            quote_id TEXT NOT NULL,
            position INTEGER NOT NULL,
            host_id TEXT NOT NULL,
            provider_id TEXT NOT NULL,
            host_owner_email TEXT NOT NULL,
            country_code TEXT,
            hostname TEXT,
            daily_rate_minor INTEGER,
            currency TEXT,
            free_duration_days INTEGER,
            offer_revision INTEGER NOT NULL,
            UNIQUE(quote_id, position),
            UNIQUE(quote_id, host_id)
        );
CREATE TABLE client_market_batches (
            id TEXT PRIMARY KEY,
            quote_id TEXT NOT NULL UNIQUE,
            client_user_id TEXT NOT NULL,
            client_owner_email TEXT NOT NULL,
            status TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );
CREATE TABLE client_market_subscriptions (
            installation_id TEXT PRIMARY KEY,
            host_id TEXT NOT NULL,
            provider_id TEXT NOT NULL,
            host_owner_email TEXT NOT NULL,
            client_user_id TEXT NOT NULL,
            client_owner_email TEXT NOT NULL,
            status TEXT NOT NULL,
            daily_rate_minor INTEGER,
            currency TEXT,
            free_duration_days INTEGER,
            offer_revision INTEGER NOT NULL,
            activated_at TEXT,
            expires_at TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            released_at TEXT
        );
CREATE UNIQUE INDEX idx_client_market_active_subscription_host
            ON client_market_subscriptions(host_id) WHERE status != 'released';
CREATE TABLE client_market_audit_events (
            id TEXT PRIMARY KEY,
            installation_id TEXT,
            host_id TEXT,
            actor_user_id TEXT,
            actor_email TEXT,
            event_type TEXT NOT NULL,
            detail_json TEXT NOT NULL DEFAULT '{}',
            created_at TEXT NOT NULL
        );
CREATE TABLE client_market_offer_events (
            id TEXT PRIMARY KEY,
            host_id TEXT NOT NULL,
            offer_revision INTEGER NOT NULL,
            actor_user_id TEXT,
            actor_email TEXT,
            detail_json TEXT NOT NULL,
            created_at TEXT NOT NULL,
            UNIQUE(host_id, offer_revision)
        );
CREATE TABLE client_market_subscription_events (
            id TEXT PRIMARY KEY,
            installation_id TEXT NOT NULL,
            host_id TEXT,
            actor_user_id TEXT,
            actor_email TEXT,
            event_type TEXT NOT NULL,
            detail_json TEXT NOT NULL,
            created_at TEXT NOT NULL
        );
CREATE INDEX idx_market_payment_methods_profile
            ON account_payment_methods(profile_user_id, enabled, position);
CREATE INDEX idx_market_subscription_events_installation
            ON client_market_subscription_events(installation_id, created_at);
CREATE TABLE client_market_recovery_state (
            installation_id TEXT PRIMARY KEY,
            host_id TEXT NOT NULL UNIQUE,
            observed_state TEXT NOT NULL
                CHECK (observed_state IN ('online', 'offline', 'stabilizing', 'blocked', 'paused')),
            incident_started_at TEXT,
            stable_since TEXT,
            attempt_level INTEGER NOT NULL DEFAULT 0
                CHECK (attempt_level >= 0 AND attempt_level <= 5),
            next_attempt_at TEXT,
            last_attempt_at TEXT,
            last_outcome TEXT,
            blocked_reason TEXT,
            paused_at TEXT,
            updated_at TEXT NOT NULL,
            FOREIGN KEY (installation_id) REFERENCES installations(id) ON DELETE CASCADE,
            FOREIGN KEY (host_id) REFERENCES router_ssh_hosts(id) ON DELETE CASCADE
        );
CREATE INDEX idx_client_market_recovery_due
            ON client_market_recovery_state(observed_state, next_attempt_at);
CREATE TABLE client_market_cleanup_recovery_state (
            host_id TEXT PRIMARY KEY,
            installation_id TEXT NOT NULL UNIQUE,
            attempt_count INTEGER NOT NULL DEFAULT 0
                CHECK (attempt_count >= 0 AND attempt_count <= 5),
            next_attempt_at TEXT,
            last_attempt_at TEXT,
            last_outcome TEXT,
            stopped_at TEXT,
            updated_at TEXT NOT NULL,
            FOREIGN KEY (host_id) REFERENCES router_ssh_hosts(id) ON DELETE CASCADE,
            FOREIGN KEY (installation_id) REFERENCES installations(id) ON DELETE CASCADE
        );
CREATE INDEX idx_client_market_cleanup_recovery_due
            ON client_market_cleanup_recovery_state(next_attempt_at)
            WHERE stopped_at IS NULL;
CREATE INDEX idx_market_quotes_owner
            ON client_market_allocation_quotes(client_user_id, status, expires_at);
CREATE INDEX idx_market_subscriptions_client
            ON client_market_subscriptions(client_user_id, status);
CREATE INDEX idx_market_subscriptions_provider
            ON client_market_subscriptions(provider_id, status);
CREATE INDEX idx_router_ssh_hosts_provider_supply
            ON router_ssh_hosts(provider_id, status, country_code);
CREATE INDEX idx_client_market_free_expiry
            ON client_market_subscriptions(status, expires_at)
            WHERE daily_rate_minor IS NULL AND expires_at IS NOT NULL;
CREATE TABLE share_market_listings (
            id TEXT PRIMARY KEY,
            share_id TEXT NOT NULL,
            installation_id TEXT NOT NULL,
            owner_user_id TEXT NOT NULL,
            owner_email TEXT NOT NULL,
            status TEXT NOT NULL CHECK (status IN ('active', 'closed')),
            deleted_at TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );
CREATE UNIQUE INDEX idx_share_market_active_listing
            ON share_market_listings(share_id) WHERE status = 'active';
CREATE TABLE share_market_seats (
            id TEXT PRIMARY KEY,
            listing_id TEXT NOT NULL,
            position INTEGER NOT NULL,
            status TEXT NOT NULL CHECK (status IN ('available', 'reserved', 'occupied', 'revoking', 'disabled', 'deleted')),
            parallel_limit INTEGER,
            token_limit INTEGER,
            token_period_json TEXT NOT NULL,
            daily_rate_minor INTEGER,
            currency TEXT,
            service_duration_days INTEGER,
            offer_revision INTEGER NOT NULL DEFAULT 1,
            current_subscription_id TEXT,
            retired_subscription_id TEXT,
            retired_at TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            UNIQUE(listing_id, position),
            FOREIGN KEY(listing_id) REFERENCES share_market_listings(id) ON DELETE CASCADE
        );
CREATE TABLE share_market_subscriptions (
            id TEXT PRIMARY KEY,
            seat_id TEXT NOT NULL,
            listing_id TEXT NOT NULL,
            share_id TEXT NOT NULL,
            installation_id TEXT NOT NULL,
            entitlement_id TEXT NOT NULL UNIQUE,
            owner_user_id TEXT NOT NULL,
            owner_email TEXT NOT NULL,
            renter_user_id TEXT NOT NULL,
            renter_email TEXT NOT NULL,
            status TEXT NOT NULL,
            parallel_limit INTEGER,
            token_limit INTEGER,
            token_period_json TEXT NOT NULL,
            daily_rate_minor INTEGER,
            currency TEXT,
            service_duration_days INTEGER,
            offer_revision INTEGER NOT NULL,
            release_reason TEXT,
            activated_at TEXT,
            expires_at TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            released_at TEXT,
            FOREIGN KEY(seat_id) REFERENCES share_market_seats(id),
            FOREIGN KEY(listing_id) REFERENCES share_market_listings(id)
        );
CREATE UNIQUE INDEX idx_share_market_active_seat
            ON share_market_subscriptions(seat_id) WHERE status NOT IN ('released', 'grant_failed');
CREATE UNIQUE INDEX idx_share_market_active_renter_share
            ON share_market_subscriptions(renter_user_id, share_id)
            WHERE status NOT IN ('released', 'grant_failed');
CREATE TABLE share_control_operations (
            id TEXT PRIMARY KEY,
            share_id TEXT NOT NULL,
            share_sequence INTEGER NOT NULL,
            entitlement_id TEXT NOT NULL,
            subscription_id TEXT NOT NULL,
            action TEXT NOT NULL CHECK (action IN ('upsert', 'revoke')),
            email TEXT NOT NULL,
            policy_json TEXT,
            status TEXT NOT NULL CHECK (status IN ('pending', 'dispatched', 'applied', 'rejected')),
            edit_id TEXT UNIQUE,
            attempts INTEGER NOT NULL DEFAULT 0,
            last_error TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            applied_at TEXT,
            UNIQUE(share_id, share_sequence),
            FOREIGN KEY(subscription_id) REFERENCES share_market_subscriptions(id)
        );
CREATE INDEX idx_share_control_dispatch
            ON share_control_operations(status, share_id, share_sequence);
CREATE TABLE share_market_events (
            id TEXT PRIMARY KEY,
            share_id TEXT NOT NULL,
            installation_id TEXT NOT NULL,
            listing_id TEXT,
            seat_id TEXT,
            subscription_id TEXT,
            actor_user_id TEXT,
            actor_email TEXT,
            event_type TEXT NOT NULL,
            dedupe_key TEXT NOT NULL UNIQUE,
            detail_json TEXT NOT NULL DEFAULT '{}',
            created_at TEXT NOT NULL
        );
CREATE INDEX idx_share_market_subscriptions_owner
            ON share_market_subscriptions(owner_user_id, status);
CREATE INDEX idx_share_market_subscriptions_renter
            ON share_market_subscriptions(renter_user_id, status);
CREATE INDEX idx_share_market_seats_listing_lifecycle
             ON share_market_seats(listing_id, retired_at, position);
CREATE TABLE supplier_billing_profiles (
            supplier_user_id TEXT NOT NULL,
            supplier_email TEXT NOT NULL,
            currency TEXT NOT NULL,
            settlement_grace_hours INTEGER NOT NULL DEFAULT 24,
            revision INTEGER NOT NULL DEFAULT 1,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            PRIMARY KEY (supplier_user_id, currency)
        );
CREATE TABLE market_credit_accounts (
            id TEXT PRIMARY KEY,
            buyer_user_id TEXT NOT NULL,
            buyer_email TEXT NOT NULL,
            supplier_user_id TEXT NOT NULL,
            supplier_email TEXT NOT NULL,
            currency TEXT NOT NULL,
            status TEXT NOT NULL,
            balance_units INTEGER NOT NULL DEFAULT 0,
            open_invoice_id TEXT,
            close_requested INTEGER NOT NULL DEFAULT 0,
            credit_kind TEXT NOT NULL CHECK (credit_kind IN ('none', 'limited', 'unlimited')),
            credit_limit_minor INTEGER,
            credit_source TEXT NOT NULL CHECK (credit_source IN ('counterparty', 'public')),
            credit_revision INTEGER NOT NULL,
            version INTEGER NOT NULL DEFAULT 1,
            last_warning_at TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            UNIQUE (buyer_user_id, supplier_user_id, currency)
        );
CREATE TABLE market_service_contracts (
            id TEXT PRIMARY KEY,
            account_id TEXT NOT NULL,
            product_kind TEXT NOT NULL,
            product_ref TEXT NOT NULL,
            service_ref TEXT NOT NULL,
            service_label TEXT NOT NULL,
            buyer_user_id TEXT NOT NULL,
            buyer_email TEXT NOT NULL,
            supplier_user_id TEXT NOT NULL,
            supplier_email TEXT NOT NULL,
            currency TEXT NOT NULL,
            daily_rate_minor INTEGER NOT NULL,
            offer_revision INTEGER NOT NULL,
            status TEXT NOT NULL,
            trial_seconds_remaining INTEGER NOT NULL,
            health_state TEXT NOT NULL DEFAULT 'unknown',
            desired_control_state TEXT NOT NULL DEFAULT 'active',
            applied_control_state TEXT NOT NULL DEFAULT 'active',
            control_error TEXT,
            last_evaluated_at TEXT NOT NULL,
            activated_at TEXT NOT NULL,
            suspended_at TEXT,
            terminated_at TEXT,
            termination_reason TEXT,
            replacement_of TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );
CREATE UNIQUE INDEX uq_market_active_product_contract
            ON market_service_contracts(product_kind, product_ref)
            WHERE status != 'terminated';
CREATE INDEX idx_market_contract_account_status
            ON market_service_contracts(account_id, status);
CREATE INDEX idx_market_contract_reconcile
            ON market_service_contracts(status, last_evaluated_at);
CREATE TABLE market_service_intervals (
            id TEXT PRIMARY KEY,
            contract_id TEXT NOT NULL,
            state TEXT NOT NULL,
            observation_reason TEXT NOT NULL,
            started_at TEXT NOT NULL,
            ended_at TEXT NOT NULL,
            elapsed_seconds INTEGER NOT NULL DEFAULT 0,
            trial_seconds INTEGER NOT NULL DEFAULT 0,
            billable_seconds INTEGER NOT NULL DEFAULT 0,
            amount_units INTEGER NOT NULL DEFAULT 0,
            invoice_id TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );
CREATE INDEX idx_market_intervals_contract
            ON market_service_intervals(contract_id, started_at);
CREATE TABLE market_accrual_entries (
            id TEXT PRIMARY KEY,
            account_id TEXT NOT NULL,
            contract_id TEXT NOT NULL,
            interval_id TEXT NOT NULL UNIQUE,
            currency TEXT NOT NULL,
            daily_rate_minor INTEGER NOT NULL,
            billable_seconds INTEGER NOT NULL,
            amount_units INTEGER NOT NULL,
            status TEXT NOT NULL,
            invoice_id TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );
CREATE INDEX idx_market_accrual_account_status
            ON market_accrual_entries(account_id, status, created_at);
CREATE TABLE market_invoices (
            id TEXT PRIMARY KEY,
            account_id TEXT NOT NULL,
            sequence INTEGER NOT NULL,
            amount_minor INTEGER NOT NULL,
            amount_cny_minor INTEGER NOT NULL,
            usd_cny_rate_micros INTEGER NOT NULL,
            amount_units INTEGER NOT NULL,
            currency TEXT NOT NULL,
            payment_methods_json TEXT NOT NULL,
            payment_contacts_json TEXT NOT NULL,
            payment_profile_updated_at TEXT NOT NULL,
            status TEXT NOT NULL,
            due_at TEXT NOT NULL,
            deadline_at TEXT NOT NULL,
            opened_at TEXT NOT NULL,
            declared_at TEXT,
            paid_at TEXT,
            overdue_at TEXT,
            disputed_at TEXT,
            voided_at TEXT,
            UNIQUE (account_id, sequence)
        );
CREATE UNIQUE INDEX uq_market_account_open_invoice
            ON market_invoices(account_id)
            WHERE status IN ('open', 'payment_declared', 'overdue', 'disputed');
CREATE TABLE market_invoice_lines (
            id TEXT PRIMARY KEY,
            invoice_id TEXT NOT NULL,
            contract_id TEXT NOT NULL,
            product_kind TEXT NOT NULL,
            product_ref TEXT NOT NULL,
            service_ref TEXT NOT NULL,
            service_label TEXT NOT NULL,
            daily_rate_minor INTEGER NOT NULL,
            billable_seconds INTEGER NOT NULL,
            amount_minor INTEGER NOT NULL,
            amount_cny_minor INTEGER NOT NULL,
            amount_units INTEGER NOT NULL,
            service_started_at TEXT NOT NULL,
            service_ended_at TEXT NOT NULL,
            evidence_json TEXT NOT NULL,
            created_at TEXT NOT NULL
        );
CREATE INDEX idx_market_invoice_lines_invoice
            ON market_invoice_lines(invoice_id, service_started_at);
CREATE TABLE market_payment_declarations (
            id TEXT PRIMARY KEY,
            invoice_id TEXT NOT NULL,
            buyer_user_id TEXT NOT NULL,
            status TEXT NOT NULL,
            payment_method_kind TEXT,
            payment_reference TEXT,
            note TEXT,
            evidence_url TEXT,
            declared_at TEXT NOT NULL,
            rejected_at TEXT,
            rejection_reason TEXT
        );
CREATE UNIQUE INDEX uq_market_active_payment_declaration
            ON market_payment_declarations(invoice_id)
            WHERE status = 'declared';
CREATE TABLE market_payment_receipts (
            id TEXT PRIMARY KEY,
            invoice_id TEXT NOT NULL UNIQUE,
            declaration_id TEXT NOT NULL,
            supplier_user_id TEXT NOT NULL,
            confirmed_at TEXT NOT NULL
        );
CREATE TABLE market_billing_disputes (
            id TEXT PRIMARY KEY,
            invoice_id TEXT NOT NULL,
            opened_by_user_id TEXT NOT NULL,
            reason TEXT NOT NULL,
            status TEXT NOT NULL,
            resolution TEXT,
            created_at TEXT NOT NULL,
            resolved_at TEXT
        );
CREATE UNIQUE INDEX uq_market_dispute_invoice
            ON market_billing_disputes(invoice_id);
CREATE TABLE market_credit_restrictions (
            id TEXT PRIMARY KEY,
            buyer_user_id TEXT NOT NULL,
            invoice_id TEXT NOT NULL UNIQUE,
            reason TEXT NOT NULL,
            status TEXT NOT NULL,
            created_at TEXT NOT NULL,
            lifted_at TEXT
        );
CREATE INDEX idx_market_restrictions_buyer
            ON market_credit_restrictions(buyer_user_id, status);
CREATE TABLE market_billing_events (
            id TEXT PRIMARY KEY,
            account_id TEXT,
            contract_id TEXT,
            invoice_id TEXT,
            actor_user_id TEXT,
            event_type TEXT NOT NULL,
            detail_json TEXT NOT NULL,
            idempotency_key TEXT NOT NULL UNIQUE,
            created_at TEXT NOT NULL
        );
CREATE INDEX idx_market_billing_events_account
            ON market_billing_events(account_id, created_at);
CREATE TABLE installations (
            id TEXT PRIMARY KEY,
            public_key TEXT NOT NULL,
            platform TEXT NOT NULL,
            app_version TEXT NOT NULL,
            owner_email TEXT,
            owner_verified_at TEXT,
            last_seen_ip TEXT,
            country_code TEXT,
            country TEXT,
            region TEXT,
            city TEXT,
            latitude REAL,
            longitude REAL,
            geo_candidate_country_code TEXT,
            geo_candidate_country TEXT,
            geo_candidate_region TEXT,
            geo_candidate_city TEXT,
            geo_candidate_latitude REAL,
            geo_candidate_longitude REAL,
            geo_candidate_hits INTEGER NOT NULL DEFAULT 0,
            geo_candidate_first_seen_at TEXT,
            geo_last_changed_at TEXT,
            created_at TEXT NOT NULL,
            last_seen_at TEXT NOT NULL,
            client_activated_at TEXT,
            control_secret_b64 TEXT NOT NULL,
            lifecycle TEXT NOT NULL DEFAULT 'active' CHECK(lifecycle IN ('active', 'fenced')),
            fenced_at TEXT,
            fence_reason TEXT
        , delegate_upgrade_to_router_owner INTEGER, app_commit_id TEXT, update_available INTEGER, upgrade_capable INTEGER, status_reported_at TEXT, public_ip TEXT, provision_source TEXT, provision_host_id TEXT, log_collection_enabled INTEGER NOT NULL DEFAULT 0 CHECK(log_collection_enabled IN (0, 1)), log_collection_reported_at TEXT);
CREATE TABLE auth_devices (
            id TEXT PRIMARY KEY,
            public_key TEXT NOT NULL UNIQUE,
            kind TEXT NOT NULL CHECK(kind IN ('browser', 'service')),
            platform TEXT NOT NULL,
            app_version TEXT NOT NULL,
            status TEXT NOT NULL DEFAULT 'active' CHECK(status IN ('active', 'revoked')),
            created_at TEXT NOT NULL,
            last_seen_at TEXT NOT NULL
        );
CREATE TABLE auth_device_nonces (
            auth_device_id TEXT NOT NULL,
            action TEXT NOT NULL,
            nonce TEXT NOT NULL,
            created_at TEXT NOT NULL,
            PRIMARY KEY (auth_device_id, action, nonce),
            FOREIGN KEY (auth_device_id) REFERENCES auth_devices(id) ON DELETE CASCADE
        );
CREATE TABLE identity_registration_nonces (
            identity_kind TEXT NOT NULL CHECK(identity_kind IN ('client_installation', 'auth_device')),
            public_key_hash TEXT NOT NULL,
            nonce TEXT NOT NULL,
            created_at TEXT NOT NULL,
            PRIMARY KEY (identity_kind, public_key_hash, nonce)
        );
CREATE TABLE installation_fences (
            public_key TEXT PRIMARY KEY,
            installation_id TEXT NOT NULL,
            takeover_id TEXT NOT NULL,
            reason TEXT NOT NULL,
            created_at TEXT NOT NULL
        );
CREATE TABLE client_subdomain_takeovers (
            id TEXT PRIMARY KEY,
            target_installation_id TEXT NOT NULL,
            source_installation_id TEXT NOT NULL,
            owner_email TEXT NOT NULL,
            target_original_subdomain TEXT NOT NULL,
            adopted_subdomain TEXT NOT NULL,
            source_retired_subdomain TEXT NOT NULL,
            status TEXT NOT NULL CHECK(status IN (
                'preparing', 'server_prepared', 'router_committed',
                'server_committed', 'completed', 'failed'
            )),
            activate_at_ms INTEGER NOT NULL,
            old_routes_json TEXT NOT NULL DEFAULT '[]',
            new_routes_json TEXT NOT NULL DEFAULT '[]',
            last_error TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            server_prepared_at TEXT,
            router_committed_at TEXT,
            server_committed_at TEXT,
            completed_at TEXT
        );
CREATE UNIQUE INDEX idx_client_subdomain_takeovers_active_target
            ON client_subdomain_takeovers(target_installation_id)
            WHERE status NOT IN ('completed', 'failed');
CREATE UNIQUE INDEX idx_client_subdomain_takeovers_active_source
            ON client_subdomain_takeovers(source_installation_id)
            WHERE status NOT IN ('completed', 'failed');
CREATE INDEX idx_client_subdomain_takeovers_recovery
            ON client_subdomain_takeovers(status, updated_at);
CREATE TABLE registration_admission_events (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            source_scope TEXT NOT NULL,
            occurred_at_ms INTEGER NOT NULL
        );
CREATE TABLE auth_device_registration_events (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            source_scope TEXT NOT NULL,
            occurred_at_ms INTEGER NOT NULL
        );
CREATE TABLE registration_geo_cache (
            source_scope TEXT PRIMARY KEY,
            status TEXT NOT NULL,
            cached_at_ms INTEGER,
            claim_owner TEXT,
            claim_expires_at_ms INTEGER,
            country_code TEXT,
            country TEXT,
            region TEXT,
            city TEXT,
            latitude REAL,
            longitude REAL,
            updated_at_ms INTEGER NOT NULL
        );
CREATE TABLE leases (
            id TEXT PRIMARY KEY,
            installation_id TEXT NOT NULL,
            connection_id TEXT NOT NULL UNIQUE,
            protocol_epoch TEXT NOT NULL,
            router_id TEXT NOT NULL,
            route_id TEXT NOT NULL,
            rotation_id TEXT NOT NULL,
            generation INTEGER NOT NULL,
            expected_generation INTEGER NOT NULL,
            state TEXT NOT NULL DEFAULT 'issued',
            subdomain TEXT NOT NULL,
            tunnel_type TEXT NOT NULL,
            ssh_username TEXT NOT NULL,
            ssh_password TEXT NOT NULL,
            issued_at TEXT NOT NULL,
            expires_at TEXT NOT NULL,
            used_at TEXT,
            ready_at TEXT,
            activated_at TEXT,
            retired_at TEXT,
            share_json TEXT
        );
CREATE TABLE tunnel_route_heads (
            router_id TEXT NOT NULL,
            route_id TEXT NOT NULL,
            subdomain TEXT NOT NULL,
            active_generation INTEGER NOT NULL,
            active_connection_id TEXT NOT NULL,
            active_rotation_id TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            PRIMARY KEY (router_id, route_id)
        );
CREATE TABLE shares (
            share_id TEXT PRIMARY KEY,
            capacity_pool_id TEXT NOT NULL,
            installation_id TEXT NOT NULL,
            share_name TEXT NOT NULL,
            owner_email TEXT,
            shared_with_emails_json TEXT NOT NULL DEFAULT '[]',
            market_access_mode TEXT NOT NULL DEFAULT 'selected',
            access_by_app_json TEXT NOT NULL DEFAULT '{}',
            app_settings_json TEXT NOT NULL DEFAULT '{}',
            for_sale_official_price_percent_by_app_json TEXT NOT NULL DEFAULT '{}',
            description TEXT,
            for_sale TEXT NOT NULL DEFAULT 'No',
            subdomain TEXT,
            app_type TEXT NOT NULL,
            provider_id TEXT,
            enabled_claude INTEGER NOT NULL DEFAULT 0,
            enabled_codex INTEGER NOT NULL DEFAULT 0,
            enabled_gemini INTEGER NOT NULL DEFAULT 0,
            token_limit INTEGER NOT NULL,
            parallel_limit INTEGER NOT NULL DEFAULT 3,
            tokens_used INTEGER NOT NULL,
            requests_count INTEGER NOT NULL,
            share_status TEXT NOT NULL,
            created_at TEXT NOT NULL,
            expires_at TEXT NOT NULL,
            upstream_provider_json TEXT,
            app_runtimes_json TEXT,
            app_providers_json TEXT,
            -- Share 的全部 app/provider 绑定快照（JSON: {app_type: provider_id}）。
            bindings_json TEXT,
            user_grants_json TEXT NOT NULL DEFAULT '{}',
            supported_user_token_periods_json TEXT NOT NULL DEFAULT '[]',
            runtime_refreshed_at TEXT,
            config_revision INTEGER NOT NULL DEFAULT 0,
            updated_at TEXT NOT NULL
        );
CREATE TABLE share_bindings (
            share_id TEXT NOT NULL REFERENCES shares(share_id) ON DELETE CASCADE,
            app_type TEXT NOT NULL CHECK (app_type IN ('claude', 'codex', 'gemini')),
            provider_id TEXT NOT NULL CHECK (provider_id != ''),
            PRIMARY KEY (share_id, app_type)
        );
CREATE INDEX idx_share_bindings_app_share
            ON share_bindings(app_type, share_id);
CREATE TABLE installation_client_tunnels (
            installation_id TEXT PRIMARY KEY,
            owner_email TEXT NOT NULL,
            subdomain TEXT NOT NULL UNIQUE,
            enabled INTEGER NOT NULL DEFAULT 1,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            last_seen_at TEXT
        );
CREATE TABLE share_request_logs (
            request_id TEXT PRIMARY KEY,
            installation_id TEXT NOT NULL,
            share_id TEXT NOT NULL,
            share_name TEXT NOT NULL,
            provider_id TEXT NOT NULL,
            provider_name TEXT NOT NULL,
            app_type TEXT NOT NULL,
            model TEXT NOT NULL,
            request_model TEXT NOT NULL,
            request_agent TEXT NOT NULL DEFAULT '',
            requested_model TEXT NOT NULL DEFAULT '',
            actual_model TEXT NOT NULL DEFAULT '',
            actual_model_source TEXT NOT NULL DEFAULT '',
            requested_reasoning_effort TEXT,
            effective_reasoning_effort TEXT,
            client_service_tier TEXT,
            effective_service_tier TEXT,
            service_tier_decision TEXT,
            usage_state TEXT NOT NULL DEFAULT 'observed',
            stream_status TEXT,
            usage_revision INTEGER NOT NULL DEFAULT 0,
            status_code INTEGER NOT NULL,
            latency_ms INTEGER NOT NULL,
            first_token_ms INTEGER,
            input_tokens INTEGER NOT NULL,
            output_tokens INTEGER NOT NULL,
            cache_read_tokens INTEGER NOT NULL,
            cache_creation_tokens INTEGER NOT NULL,
            quota_tokens INTEGER,
            is_streaming INTEGER NOT NULL,
            session_id TEXT,
            user_country TEXT,
            user_country_iso3 TEXT,
            user_email TEXT,
            is_health_check INTEGER NOT NULL DEFAULT 0,
            created_at INTEGER NOT NULL
        );
CREATE TABLE image_generation_jobs (
            job_id TEXT PRIMARY KEY,
            share_id TEXT NOT NULL,
            installation_id TEXT NOT NULL,
            share_name TEXT NOT NULL,
            provider_id TEXT NOT NULL,
            provider_name TEXT NOT NULL,
            app_type TEXT NOT NULL,
            model TEXT NOT NULL,
            status TEXT NOT NULL,
            status_code INTEGER,
            latency_ms INTEGER NOT NULL DEFAULT 0,
            queued_at INTEGER NOT NULL,
            started_at INTEGER,
            completed_at INTEGER,
            expires_at INTEGER,
            prompt_preview TEXT,
            error_message TEXT,
            result_mime_type TEXT,
            result_size_bytes INTEGER,
            result_storage_key TEXT,
            result_token_hash TEXT,
            created_by_email TEXT,
            client_ip TEXT,
            user_country TEXT,
            idempotency_key TEXT
        );
CREATE TABLE image_generation_request_logs (
            request_id TEXT PRIMARY KEY,
            share_id TEXT NOT NULL,
            installation_id TEXT NOT NULL,
            share_name TEXT NOT NULL,
            provider_id TEXT NOT NULL,
            provider_name TEXT NOT NULL,
            app_type TEXT NOT NULL,
            model TEXT NOT NULL,
            status TEXT NOT NULL,
            status_code INTEGER,
            latency_ms INTEGER NOT NULL DEFAULT 0,
            created_at INTEGER NOT NULL,
            completed_at INTEGER,
            prompt_preview TEXT,
            error_message TEXT,
            result_mime_type TEXT,
            result_size_bytes INTEGER,
            result_storage_key TEXT,
            result_access_token TEXT,
            created_by_email TEXT,
            client_ip TEXT,
            user_country TEXT
        );
CREATE TABLE share_health_checks (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            share_id TEXT NOT NULL,
            checked_at INTEGER NOT NULL,
            is_healthy INTEGER NOT NULL,
            status TEXT NOT NULL DEFAULT 'unknown',
            reason TEXT,
            router_epoch TEXT NOT NULL DEFAULT 'legacy'
        );
CREATE TABLE installation_health_checks (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            installation_id TEXT NOT NULL,
            checked_at INTEGER NOT NULL,
            is_healthy INTEGER NOT NULL,
            status TEXT NOT NULL DEFAULT 'unknown',
            reason TEXT,
            router_epoch TEXT NOT NULL DEFAULT 'legacy'
        );
CREATE TABLE share_model_health_checks (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            request_id TEXT NOT NULL UNIQUE,
            share_id TEXT NOT NULL,
            subdomain TEXT NOT NULL,
            app_type TEXT NOT NULL,
            requested_model TEXT NOT NULL,
            actual_model TEXT NOT NULL DEFAULT '',
            status TEXT NOT NULL,
            status_code INTEGER,
            latency_ms INTEGER NOT NULL DEFAULT 0,
            first_token_ms INTEGER,
            error_message TEXT,
            checked_at INTEGER NOT NULL,
            source TEXT NOT NULL DEFAULT 'scheduled'
        );
CREATE TABLE share_model_health_state (
            share_id TEXT NOT NULL,
            app_type TEXT NOT NULL,
            requested_model TEXT NOT NULL,
            actual_model TEXT NOT NULL DEFAULT '',
            last_status TEXT NOT NULL,
            last_success_at INTEGER,
            last_failed_at INTEGER,
            last_checked_at INTEGER NOT NULL,
            recent_results_json TEXT NOT NULL DEFAULT '[]',
            error_message TEXT,
            updated_at INTEGER NOT NULL,
            PRIMARY KEY (share_id, app_type, requested_model)
        );
CREATE TABLE share_edit_requests (
            id TEXT PRIMARY KEY,
            share_id TEXT NOT NULL,
            installation_id TEXT NOT NULL,
            owner_email TEXT NOT NULL,
            revision INTEGER NOT NULL,
            status TEXT NOT NULL,
            patch_json TEXT NOT NULL,
            created_by_email TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            applied_at TEXT,
            error_message TEXT,
            retired_at TEXT
        );
CREATE TABLE market_disabled_shares (
            market_email TEXT NOT NULL,
            share_id TEXT NOT NULL,
            disabled_by_email TEXT NOT NULL,
            reason TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            PRIMARY KEY (market_email, share_id)
        );
CREATE TABLE market_share_model_failure_state (
            market_email TEXT NOT NULL,
            share_id TEXT NOT NULL,
            app_type TEXT NOT NULL,
            requested_model TEXT NOT NULL,
            actual_model TEXT NOT NULL DEFAULT '',
            last_status TEXT NOT NULL,
            last_success_at INTEGER,
            last_failed_at INTEGER,
            last_checked_at INTEGER NOT NULL,
            recent_results_json TEXT NOT NULL DEFAULT '[]',
            error_message TEXT,
            updated_at INTEGER NOT NULL,
            PRIMARY KEY (market_email, share_id, app_type, requested_model)
        );
CREATE TABLE market_share_runtime_states (
            market_email TEXT NOT NULL,
            share_id TEXT NOT NULL,
            router_id TEXT,
            scope TEXT NOT NULL,
            kind TEXT NOT NULL,
            app_type TEXT,
            model_id TEXT,
            model_name TEXT,
            reason_kind TEXT,
            reason TEXT,
            failure_count INTEGER,
            expires_at TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );
CREATE TABLE dashboard_presence (
            session_id TEXT PRIMARY KEY,
            last_seen_at INTEGER NOT NULL
        );
CREATE TABLE dashboard_ux_events (
            id TEXT PRIMARY KEY,
            event_type TEXT NOT NULL,
            source TEXT,
            target_type TEXT,
            step_count INTEGER,
            elapsed_ms INTEGER,
            keyboard INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL
        );
CREATE TABLE email_send_logs (
            id TEXT PRIMARY KEY,
            email_type TEXT NOT NULL,
            to_email TEXT NOT NULL,
            provider_message_id TEXT,
            status TEXT NOT NULL,
            error_message TEXT,
            created_at TEXT NOT NULL
        );
CREATE TABLE request_nonces (
            installation_id TEXT NOT NULL,
            action TEXT NOT NULL,
            nonce TEXT NOT NULL,
            created_at TEXT NOT NULL,
            PRIMARY KEY (installation_id, action, nonce)
        );
CREATE TABLE installation_notification_state (
            installation_id TEXT PRIMARY KEY,
            monitoring_enabled INTEGER NOT NULL DEFAULT 0,
            presence_state TEXT NOT NULL DEFAULT 'unknown',
            last_heartbeat_at TEXT,
            last_boot_id TEXT,
            offline_candidate_since TEXT,
            offline_since TEXT,
            recovery_candidate_since TEXT,
            last_recovered_at TEXT,
            offline_episode INTEGER NOT NULL DEFAULT 0,
            last_offline_event_at TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            FOREIGN KEY (installation_id) REFERENCES installations(id) ON DELETE CASCADE
        );
CREATE TABLE client_notification_events (
            id TEXT PRIMARY KEY,
            dedupe_key TEXT NOT NULL UNIQUE,
            kind TEXT NOT NULL,
            installation_id TEXT NOT NULL,
            episode INTEGER NOT NULL DEFAULT 0,
            status TEXT NOT NULL,
            occurred_at TEXT NOT NULL,
            not_before TEXT NOT NULL,
            snapshot_json TEXT NOT NULL,
            suppression_reason TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );
CREATE TABLE operator_alert_signal_outbox (
            source_event_id TEXT PRIMARY KEY,
            fingerprint TEXT NOT NULL,
            transition TEXT NOT NULL CHECK (transition IN ('firing', 'resolved')),
            kind TEXT NOT NULL,
            entity_kind TEXT NOT NULL,
            entity_id TEXT,
            severity TEXT NOT NULL CHECK (severity IN ('info', 'warning', 'critical')),
            title TEXT NOT NULL,
            message TEXT NOT NULL,
            details_json TEXT NOT NULL DEFAULT '{}',
            status TEXT NOT NULL DEFAULT 'pending'
                CHECK (status IN ('pending', 'claimed', 'retry', 'completed')),
            attempts INTEGER NOT NULL DEFAULT 0,
            next_attempt_at TEXT,
            claimed_by TEXT,
            claim_expires_at TEXT,
            last_error TEXT,
            occurred_at TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            completed_at TEXT
        );
CREATE TABLE installation_setup_completions (
            installation_id TEXT PRIMARY KEY,
            setup_id TEXT NOT NULL,
            source TEXT NOT NULL DEFAULT 'explicit'
                CHECK (source IN ('explicit', 'legacy_fallback')),
            password_hint TEXT,
            notification_status TEXT NOT NULL
                CHECK (notification_status IN ('queued', 'suppressed_disabled')),
            event_id TEXT,
            completed_at TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            FOREIGN KEY (installation_id) REFERENCES installations(id) ON DELETE CASCADE,
            FOREIGN KEY (event_id) REFERENCES client_notification_events(id) ON DELETE SET NULL
        );
CREATE TABLE email_delivery_batches (
            id TEXT PRIMARY KEY,
            notification_lane TEXT NOT NULL DEFAULT 'registration'
                CHECK (notification_lane IN ('offline', 'registration')),
            recipient TEXT NOT NULL,
            recipient_priority INTEGER NOT NULL DEFAULT 0,
            from_address TEXT NOT NULL,
            reply_to TEXT,
            subject TEXT NOT NULL,
            html_body TEXT NOT NULL,
            text_body TEXT NOT NULL DEFAULT '',
            idempotency_key TEXT NOT NULL UNIQUE,
            status TEXT NOT NULL,
            attempts INTEGER NOT NULL DEFAULT 0,
            not_before TEXT NOT NULL,
            next_attempt_at TEXT,
            claim_owner TEXT,
            claim_expires_at TEXT,
            provider_message_id TEXT,
            error_message TEXT,
            template_fingerprint TEXT NOT NULL,
            delivery_kind TEXT NOT NULL DEFAULT 'lifecycle',
            incident_key TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            sent_at TEXT
        );
CREATE TABLE email_delivery_batch_items (
            batch_id TEXT NOT NULL,
            event_id TEXT NOT NULL,
            recipient TEXT NOT NULL,
            PRIMARY KEY (batch_id, event_id, recipient),
            UNIQUE (event_id, recipient),
            FOREIGN KEY (batch_id) REFERENCES email_delivery_batches(id) ON DELETE CASCADE,
            FOREIGN KEY (event_id) REFERENCES client_notification_events(id) ON DELETE CASCADE
        );
CREATE TABLE client_notification_runtime (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            enabled INTEGER NOT NULL DEFAULT 0,
            enabled_since TEXT,
            policy_fingerprint TEXT,
            delivery_configured INTEGER NOT NULL DEFAULT 0,
            template_fingerprint TEXT,
            recipient_hourly_limit INTEGER NOT NULL DEFAULT 10,
            global_hourly_limit INTEGER NOT NULL DEFAULT 50,
            registration_recipient_hourly_limit INTEGER NOT NULL DEFAULT 3,
            registration_global_hourly_limit INTEGER NOT NULL DEFAULT 10,
            registration_overflow_active INTEGER NOT NULL DEFAULT 0,
            updated_at TEXT NOT NULL
        );
CREATE TABLE client_registration_notification_overflow (
            window_start TEXT PRIMARY KEY,
            window_end TEXT NOT NULL,
            event_count INTEGER NOT NULL DEFAULT 0,
            first_occurred_at TEXT NOT NULL,
            last_occurred_at TEXT NOT NULL,
            status TEXT NOT NULL DEFAULT 'pending'
                CHECK (status IN ('pending', 'materialized')),
            summary_event_id TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );
CREATE TABLE users (
            id TEXT PRIMARY KEY,
            email_normalized TEXT NOT NULL UNIQUE,
            status TEXT NOT NULL DEFAULT 'active',
            public_stats_enabled INTEGER NOT NULL DEFAULT 1
                CHECK (public_stats_enabled IN (0, 1)),
            created_at TEXT NOT NULL,
            last_login_at TEXT NOT NULL
        );
CREATE TABLE email_login_challenges (
            id TEXT PRIMARY KEY,
            email_normalized TEXT NOT NULL,
            auth_source_kind TEXT NOT NULL
                CHECK(auth_source_kind IN ('auth_device', 'client_installation')),
            auth_source_id TEXT NOT NULL,
            purpose TEXT NOT NULL,
            code_hash TEXT NOT NULL,
            expires_at TEXT NOT NULL,
            consumed_at TEXT,
            attempt_count INTEGER NOT NULL DEFAULT 0,
            resend_available_at TEXT NOT NULL,
            created_ip TEXT,
            created_user_agent TEXT,
            created_at TEXT NOT NULL
        );
CREATE TABLE user_sessions (
            id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL,
            auth_source_kind TEXT NOT NULL
                CHECK(auth_source_kind IN ('auth_device', 'client_installation')),
            auth_source_id TEXT NOT NULL,
            access_token_hash TEXT NOT NULL UNIQUE,
            refresh_token_hash TEXT NOT NULL UNIQUE,
            access_expires_at TEXT NOT NULL,
            refresh_expires_at TEXT NOT NULL,
            revoked_at TEXT,
            rotated_at TEXT,
            created_at TEXT NOT NULL,
            last_used_at TEXT NOT NULL
        );
CREATE TABLE user_api_tokens (
            id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL,
            name TEXT NOT NULL,
            token_hash TEXT NOT NULL UNIQUE,
            token_prefix TEXT NOT NULL,
            token_plaintext TEXT,
            scopes_json TEXT NOT NULL,
            created_at TEXT NOT NULL,
            last_used_at TEXT,
            reset_at TEXT,
            revoked_at TEXT
        );
CREATE TABLE router_markets (
            id TEXT PRIMARY KEY,
            display_name TEXT NOT NULL,
            email TEXT NOT NULL UNIQUE,
            subdomain TEXT NOT NULL UNIQUE,
            public_base_url TEXT NOT NULL,
            market_kind TEXT NOT NULL DEFAULT 'usage',
            scopes_json TEXT NOT NULL DEFAULT '["market:shares:read","market:proxy:use","market:email:notify"]',
            pricing_json TEXT,
            maintenance_enabled INTEGER NOT NULL DEFAULT 0,
            maintenance_message TEXT,
            status TEXT NOT NULL DEFAULT 'active',
            listed INTEGER NOT NULL DEFAULT 1,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            last_seen_at TEXT NOT NULL,
            offline_since TEXT
        );
CREATE TABLE router_gateways (
            id TEXT PRIMARY KEY,
            owner_email TEXT NOT NULL,
            display_name TEXT NOT NULL,
            public_key TEXT NOT NULL UNIQUE,
            public_base_url TEXT,
            app_version TEXT,
            scopes_json TEXT NOT NULL DEFAULT '["gateway:shares:read","gateway:proxy:use","gateway:feedback:write","gateway:request_logs:write"]',
            status TEXT NOT NULL DEFAULT 'active',
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            last_seen_at TEXT NOT NULL
        );
CREATE TABLE market_notification_emails (
            id TEXT PRIMARY KEY,
            market_email TEXT NOT NULL,
            kind TEXT NOT NULL,
            to_email TEXT NOT NULL,
            locale TEXT NOT NULL,
            payload_json TEXT NOT NULL,
            provider_message_id TEXT,
            status TEXT NOT NULL,
            error_message TEXT,
            created_at TEXT NOT NULL
        );
CREATE TABLE market_request_logs (
            request_id TEXT PRIMARY KEY,
            market_id TEXT NOT NULL,
            market_email TEXT NOT NULL,
            market_subdomain TEXT NOT NULL,
            user_email TEXT,
            api_key_prefix TEXT,
            router_id TEXT,
            share_id TEXT,
            share_subdomain TEXT,
            model TEXT,
            request_agent TEXT NOT NULL DEFAULT '',
            requested_model TEXT NOT NULL DEFAULT '',
            actual_model TEXT NOT NULL DEFAULT '',
            actual_model_source TEXT NOT NULL DEFAULT '',
            status TEXT NOT NULL,
            status_code INTEGER,
            error_message TEXT,
            latency_ms INTEGER,
            input_tokens INTEGER NOT NULL DEFAULT 0,
            output_tokens INTEGER NOT NULL DEFAULT 0,
            cache_read_tokens INTEGER NOT NULL DEFAULT 0,
            cache_creation_tokens INTEGER NOT NULL DEFAULT 0,
            usage_amount_usd TEXT,
            created_at TEXT NOT NULL,
            settled_at TEXT,
            user_country TEXT,
            user_country_iso3 TEXT,
            synced_at TEXT NOT NULL
        );
CREATE INDEX idx_leases_installation_id ON leases(installation_id);
CREATE INDEX idx_registration_admission_source_time
            ON registration_admission_events(source_scope, occurred_at_ms);
CREATE INDEX idx_registration_admission_time
            ON registration_admission_events(occurred_at_ms);
CREATE INDEX idx_registration_geo_cache_cached
            ON registration_geo_cache(status, cached_at_ms);
CREATE INDEX idx_registration_geo_cache_claim
            ON registration_geo_cache(status, claim_expires_at_ms);
CREATE INDEX idx_auth_device_registration_events_source_time
            ON auth_device_registration_events(source_scope, occurred_at_ms);
CREATE INDEX idx_installations_unowned
            ON installations(owner_verified_at) WHERE owner_verified_at IS NULL;
CREATE INDEX idx_installations_activated
            ON installations(lifecycle, client_activated_at, last_seen_at);
CREATE INDEX idx_auth_devices_status_seen
            ON auth_devices(status, last_seen_at);
CREATE INDEX idx_auth_device_nonces_created
            ON auth_device_nonces(created_at);
CREATE INDEX idx_identity_registration_nonces_created
            ON identity_registration_nonces(created_at);
CREATE INDEX idx_leases_subdomain ON leases(subdomain);
CREATE INDEX idx_installation_client_tunnels_owner ON installation_client_tunnels(owner_email, updated_at DESC);
CREATE INDEX idx_shares_installation_id ON shares(installation_id);
CREATE UNIQUE INDEX idx_shares_subdomain_unique ON shares(subdomain) WHERE subdomain IS NOT NULL;
CREATE INDEX idx_share_request_logs_share_id ON share_request_logs(share_id, created_at DESC);
CREATE INDEX idx_share_request_logs_share_app_user_created
            ON share_request_logs(share_id, app_type, user_email, created_at);
CREATE INDEX idx_share_request_logs_created_at ON share_request_logs(created_at);
CREATE INDEX idx_image_jobs_share_queued ON image_generation_jobs(share_id, queued_at DESC);
CREATE INDEX idx_image_jobs_provider_queued ON image_generation_jobs(provider_id, queued_at DESC);
CREATE INDEX idx_image_jobs_installation_queued ON image_generation_jobs(installation_id, queued_at DESC);
CREATE INDEX idx_image_jobs_status_queued ON image_generation_jobs(status, queued_at DESC);
CREATE INDEX idx_image_jobs_expires ON image_generation_jobs(expires_at);
CREATE INDEX idx_image_request_logs_share_created ON image_generation_request_logs(share_id, created_at DESC);
CREATE INDEX idx_image_request_logs_provider_created ON image_generation_request_logs(provider_id, created_at DESC);
CREATE INDEX idx_image_request_logs_status_created ON image_generation_request_logs(status, created_at DESC);
CREATE INDEX idx_image_request_logs_created_at ON image_generation_request_logs(created_at);
CREATE INDEX idx_share_health_checks ON share_health_checks(share_id, checked_at DESC);
CREATE INDEX idx_installation_health_checks ON installation_health_checks(installation_id, checked_at DESC);
CREATE INDEX idx_share_health_checks_checked_at ON share_health_checks(checked_at DESC);
CREATE INDEX idx_installation_health_checks_checked_at ON installation_health_checks(checked_at DESC);
CREATE INDEX idx_share_model_health_checks_share ON share_model_health_checks(share_id, app_type, requested_model, checked_at DESC);
CREATE INDEX idx_share_model_health_state_share ON share_model_health_state(share_id, app_type, last_status);
CREATE INDEX idx_dashboard_presence_last_seen ON dashboard_presence(last_seen_at DESC);
CREATE INDEX idx_installation_notification_presence
            ON installation_notification_state(monitoring_enabled, presence_state, last_heartbeat_at);
CREATE INDEX idx_client_notification_events_pending
            ON client_notification_events(status, not_before, occurred_at);
CREATE INDEX idx_client_notification_events_lane_pending
            ON client_notification_events(status, kind, not_before, occurred_at, id);
CREATE INDEX idx_client_notification_events_installation_kind_status
            ON client_notification_events(installation_id, kind, status);
CREATE INDEX idx_client_notification_events_storm
            ON client_notification_events(status, occurred_at, kind, installation_id);
CREATE INDEX idx_installation_setup_completions_event
            ON installation_setup_completions(event_id);
CREATE INDEX idx_email_delivery_batches_claim
            ON email_delivery_batches(status, next_attempt_at, not_before, claim_expires_at);
CREATE INDEX idx_email_delivery_batches_send_cap
            ON email_delivery_batches(status, sent_at);
CREATE INDEX idx_email_delivery_batches_claim_cap
            ON email_delivery_batches(status, claim_expires_at);
CREATE INDEX idx_email_delivery_batches_recipient_send_cap
            ON email_delivery_batches(LOWER(recipient), status, sent_at);
CREATE INDEX idx_email_delivery_batches_recipient_claim_cap
            ON email_delivery_batches(LOWER(recipient), status, claim_expires_at);
CREATE INDEX idx_email_delivery_batches_recent
            ON email_delivery_batches(created_at DESC, id DESC);
CREATE INDEX idx_email_delivery_batches_retention
            ON email_delivery_batches(updated_at, status);
CREATE INDEX idx_client_notification_events_retention
            ON client_notification_events(updated_at, status);
CREATE INDEX idx_operator_alert_signal_outbox_pending
            ON operator_alert_signal_outbox(status, next_attempt_at, occurred_at);
CREATE INDEX idx_operator_alert_signal_outbox_retention
            ON operator_alert_signal_outbox(status, completed_at);
CREATE INDEX idx_client_registration_notification_overflow_pending
            ON client_registration_notification_overflow(status, window_end, window_start);
CREATE INDEX idx_dashboard_ux_events_created ON dashboard_ux_events(created_at DESC);
CREATE INDEX idx_email_send_logs_created_at ON email_send_logs(created_at DESC);
CREATE INDEX idx_request_nonces_created_at ON request_nonces(created_at DESC);
CREATE INDEX idx_auth_challenges_email ON email_login_challenges(email_normalized, created_at DESC);
CREATE INDEX idx_auth_challenges_source
            ON email_login_challenges(auth_source_kind, auth_source_id, created_at DESC);
CREATE INDEX idx_user_sessions_user_id ON user_sessions(user_id, created_at DESC);
CREATE INDEX idx_user_sessions_source
            ON user_sessions(auth_source_kind, auth_source_id, created_at DESC);
CREATE INDEX idx_router_markets_status ON router_markets(status, listed, updated_at DESC);
CREATE INDEX idx_router_markets_subdomain ON router_markets(subdomain);
CREATE INDEX idx_router_gateways_owner ON router_gateways(owner_email, updated_at DESC);
CREATE INDEX idx_router_gateways_status ON router_gateways(status, updated_at DESC);
CREATE INDEX idx_market_notification_emails_market_created ON market_notification_emails(market_email, created_at DESC);
CREATE INDEX idx_market_notification_emails_to_created ON market_notification_emails(to_email, created_at DESC);
CREATE INDEX idx_market_request_logs_market_created ON market_request_logs(market_email, created_at DESC);
CREATE INDEX idx_market_request_logs_share_created ON market_request_logs(share_id, created_at DESC);
CREATE INDEX idx_market_request_logs_share_agent_user_created
            ON market_request_logs(share_id, request_agent, user_email, created_at);
CREATE INDEX idx_market_request_logs_created ON market_request_logs(created_at DESC);
CREATE INDEX idx_market_share_model_failure_state_share ON market_share_model_failure_state(market_email, share_id, app_type, last_status);
CREATE INDEX idx_market_share_runtime_states_market_share ON market_share_runtime_states(market_email, share_id);
CREATE INDEX idx_market_share_runtime_states_expires ON market_share_runtime_states(expires_at);
CREATE UNIQUE INDEX idx_market_share_runtime_states_unique
            ON market_share_runtime_states (
                market_email,
                share_id,
                scope,
                kind,
                COALESCE(app_type, ''),
                COALESCE(model_id, ''),
                COALESCE(model_name, '')
            );
CREATE TABLE admin_audit_log (
            id TEXT PRIMARY KEY,
            actor_email TEXT,
            action TEXT NOT NULL,
            payload_json TEXT,
            ip TEXT,
            created_at TEXT NOT NULL
        );
CREATE INDEX idx_admin_audit_created ON admin_audit_log(created_at DESC);
CREATE TABLE router_map_display_settings (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            settings_json TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );
CREATE TABLE router_announcement_settings (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            settings_json TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );
CREATE INDEX idx_share_request_logs_user_email_created
            ON share_request_logs(user_email, created_at)
            WHERE user_email IS NOT NULL AND is_health_check = 0;
CREATE INDEX idx_market_request_logs_user_email_created
            ON market_request_logs(user_email, created_at)
            WHERE user_email IS NOT NULL;
CREATE INDEX idx_leases_route_generation
         ON leases(route_id, generation);
CREATE INDEX idx_leases_rotation
         ON leases(route_id, rotation_id);
CREATE INDEX idx_share_edit_requests_share_status ON share_edit_requests(share_id, status, revision DESC);
CREATE UNIQUE INDEX idx_share_edit_requests_pending_unique ON share_edit_requests(share_id) WHERE status = 'pending';
CREATE INDEX idx_market_disabled_shares_share ON market_disabled_shares(share_id);
CREATE INDEX idx_email_delivery_batches_priority_claim
         ON email_delivery_batches(
             notification_lane, recipient_priority, status, next_attempt_at, not_before,
             claim_expires_at
         );
CREATE INDEX idx_email_delivery_batches_lane_claim
             ON email_delivery_batches(
                 notification_lane, status, next_attempt_at, not_before,
                 claim_expires_at, recipient_priority, created_at, id
             );
CREATE INDEX idx_email_delivery_batches_lane_send_cap
             ON email_delivery_batches(notification_lane, status, sent_at);
CREATE INDEX idx_email_delivery_batches_lane_claim_cap
             ON email_delivery_batches(notification_lane, status, claim_expires_at);
CREATE INDEX idx_email_delivery_batches_recipient_lane_send_cap
             ON email_delivery_batches(
                 LOWER(recipient), notification_lane, status, sent_at
             );
CREATE INDEX idx_email_delivery_batches_recipient_lane_claim_cap
             ON email_delivery_batches(
                 LOWER(recipient), notification_lane, status, claim_expires_at
             );
CREATE INDEX idx_email_delivery_batches_incident
         ON email_delivery_batches(recipient, incident_key, created_at DESC);
CREATE TABLE chat_rooms (
            id TEXT PRIMARY KEY,
            installation_id TEXT NOT NULL,
            client_label_snapshot TEXT NOT NULL,
            owner_user_id_snapshot TEXT,
            owner_email_snapshot TEXT NOT NULL,
            owner_generation INTEGER NOT NULL DEFAULT 1,
            status TEXT NOT NULL DEFAULT 'active'
                CHECK (status IN ('active', 'archived')),
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            last_message_at TEXT,
            archived_at TEXT,
            delete_after TEXT,
            UNIQUE (installation_id, owner_generation)
        );
CREATE TABLE chat_messages (
            seq INTEGER PRIMARY KEY AUTOINCREMENT,
            id TEXT NOT NULL UNIQUE,
            room_id TEXT NOT NULL,
            author_user_id TEXT NOT NULL,
            author_email TEXT NOT NULL,
            author_label TEXT NOT NULL,
            client_message_id TEXT NOT NULL,
            author_kind TEXT NOT NULL DEFAULT 'user'
                CHECK (author_kind IN ('user', 'system')),
            message_kind TEXT NOT NULL DEFAULT 'text'
                CHECK (message_kind IN ('text', 'market_event')),
            event_type TEXT,
            event_payload_json TEXT,
            payload_version INTEGER NOT NULL DEFAULT 1,
            source_event_id TEXT UNIQUE,
            body TEXT NOT NULL,
            status TEXT NOT NULL DEFAULT 'visible'
                CHECK (status IN ('visible', 'deleted')),
            deleted_by TEXT,
            deleted_at TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            UNIQUE (room_id, author_user_id, client_message_id),
            FOREIGN KEY (room_id) REFERENCES chat_rooms(id) ON DELETE CASCADE
        );
CREATE TABLE chat_visits (
            user_id TEXT NOT NULL,
            room_id TEXT NOT NULL,
            first_opened_at TEXT NOT NULL,
            last_opened_at TEXT NOT NULL,
            last_read_seq INTEGER NOT NULL DEFAULT 0,
            updated_at TEXT NOT NULL,
            PRIMARY KEY (user_id, room_id),
            FOREIGN KEY (room_id) REFERENCES chat_rooms(id) ON DELETE CASCADE
        );
CREATE TABLE share_presence_state (
            share_id TEXT PRIMARY KEY,
            state TEXT NOT NULL DEFAULT 'unknown'
                CHECK (state IN ('unknown', 'online', 'offline')),
            router_epoch TEXT NOT NULL,
            has_online_baseline INTEGER NOT NULL DEFAULT 0,
            unhealthy_since TEXT,
            healthy_since TEXT,
            offline_episode INTEGER NOT NULL DEFAULT 0,
            last_offline_event_id TEXT,
            last_recovered_event_id TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );
CREATE TABLE client_chat_system_outbox (
            id TEXT PRIMARY KEY,
            installation_id TEXT NOT NULL,
            source_kind TEXT NOT NULL,
            source_event_id TEXT NOT NULL,
            event_type TEXT NOT NULL,
            payload_json TEXT NOT NULL,
            follower_user_ids_json TEXT NOT NULL DEFAULT '[]',
            status TEXT NOT NULL DEFAULT 'pending'
                CHECK (status IN ('pending', 'processing', 'completed', 'dead_letter')),
            attempts INTEGER NOT NULL DEFAULT 0,
            next_attempt_at TEXT,
            last_error TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            completed_at TEXT,
            UNIQUE (source_kind, source_event_id, installation_id)
        );
CREATE TABLE chat_public_payment_assets (
            asset_id TEXT NOT NULL,
            message_id TEXT NOT NULL,
            room_id TEXT NOT NULL,
            created_at TEXT NOT NULL,
            PRIMARY KEY (asset_id, message_id),
            FOREIGN KEY (message_id) REFERENCES chat_messages(id) ON DELETE CASCADE,
            FOREIGN KEY (room_id) REFERENCES chat_rooms(id) ON DELETE CASCADE
        );
CREATE TABLE chat_rate_limit (
            scope TEXT NOT NULL,
            bucket_start INTEGER NOT NULL,
            count INTEGER NOT NULL,
            PRIMARY KEY (scope, bucket_start)
        );
CREATE TABLE chat_email_events (
            id TEXT PRIMARY KEY,
            message_id TEXT NOT NULL UNIQUE,
            room_id TEXT NOT NULL,
            installation_id TEXT NOT NULL,
            owner_generation INTEGER NOT NULL,
            recipient TEXT NOT NULL,
            status TEXT NOT NULL,
            window_started_at TEXT NOT NULL,
            window_ends_at TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            FOREIGN KEY (message_id) REFERENCES chat_messages(id) ON DELETE CASCADE,
            FOREIGN KEY (room_id) REFERENCES chat_rooms(id) ON DELETE CASCADE
        );
CREATE TABLE chat_email_deliveries (
            id TEXT PRIMARY KEY,
            room_id TEXT NOT NULL,
            installation_id TEXT NOT NULL,
            client_label TEXT NOT NULL,
            owner_generation INTEGER NOT NULL,
            recipient TEXT NOT NULL,
            from_address TEXT NOT NULL,
            reply_to TEXT,
            subject TEXT NOT NULL,
            html_body TEXT NOT NULL,
            text_body TEXT NOT NULL,
            idempotency_key TEXT NOT NULL UNIQUE,
            status TEXT NOT NULL,
            attempts INTEGER NOT NULL DEFAULT 0,
            not_before TEXT NOT NULL,
            next_attempt_at TEXT,
            claim_owner TEXT,
            claim_expires_at TEXT,
            provider_message_id TEXT,
            error_message TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            sent_at TEXT,
            FOREIGN KEY (room_id) REFERENCES chat_rooms(id) ON DELETE CASCADE
        );
CREATE TABLE chat_email_delivery_items (
            delivery_id TEXT NOT NULL,
            event_id TEXT NOT NULL,
            PRIMARY KEY (delivery_id, event_id),
            FOREIGN KEY (delivery_id) REFERENCES chat_email_deliveries(id) ON DELETE CASCADE,
            FOREIGN KEY (event_id) REFERENCES chat_email_events(id) ON DELETE CASCADE
        );
CREATE INDEX idx_chat_rooms_status
            ON chat_rooms(status, delete_after);
CREATE UNIQUE INDEX idx_chat_rooms_active_installation
            ON chat_rooms(installation_id) WHERE status = 'active';
CREATE INDEX idx_chat_messages_room_seq
            ON chat_messages(room_id, seq DESC);
CREATE INDEX idx_chat_messages_author
            ON chat_messages(author_user_id, created_at DESC);
CREATE INDEX idx_chat_visits_user_recent
            ON chat_visits(user_id, last_opened_at DESC);
CREATE INDEX idx_client_chat_system_outbox_pending
            ON client_chat_system_outbox(status, next_attempt_at, created_at);
CREATE INDEX idx_chat_public_payment_assets_asset
            ON chat_public_payment_assets(asset_id);
CREATE INDEX idx_chat_events_pending
            ON chat_email_events(status, window_ends_at, room_id, owner_generation);
CREATE INDEX idx_chat_delivery_claim
            ON chat_email_deliveries(status, next_attempt_at, not_before, claim_expires_at);
CREATE INDEX idx_chat_delivery_recent
            ON chat_email_deliveries(created_at DESC, id DESC);
CREATE UNIQUE INDEX idx_installations_public_key_unique
         ON installations(public_key);
