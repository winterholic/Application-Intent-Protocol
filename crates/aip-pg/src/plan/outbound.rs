//! `outbound webhooks for E e { events [...] where cond sign hmac_sha256 retry N over D disable after D failing }`
//! lowers to one handler per listed event and an [`Outbound`] the runtime delivers from.
//!
//! The handler runs in the transaction that dispatches the event (so an event is queued once per endpoint even if the
//! dispatch is repeated) and writes one row per matching endpoint into `_aip_outbound_delivery`. The endpoints are the
//! rows of `E` that satisfy the `where` condition (over the endpoint row and the event), have a URL, are not disabled,
//! and belong to the tenant the event names: a condition that forgets the tenant cannot send an event to another
//! tenant's endpoint. Sending, signing, retrying and disabling happen in the runtime (`aip-runtime/src/outbound.rs`).

use super::Planner;
use crate::names::{lit, q};
use crate::schema::Col;
use crate::sqlexpr::{Val, marker};
use crate::tenant::{self as tn, TenantScope};
use aip_ir as ir;
use aip_ir::tenant;
use aip_plan::*;

impl Planner<'_> {
    pub fn outbound(&mut self, o: &ir::OutboundWebhooks) -> (Vec<Handler>, Option<Outbound>) {
        let t = self.s.table(&o.entity).clone();
        let Some(Col::Scalar { col: url_col, .. }) = t.col("url").cloned() else {
            self.fail(format!("{} has no 'url' column to deliver to", o.entity));
            return (Vec::new(), None);
        };
        let mut handlers = Vec::new();
        for event in &o.events {
            let mut c = self.compiler();
            c.push();
            let a = c.fresh_alias();
            c.bind(&o.alias, Val::Row { entity: o.entity.clone(), alias: a.clone() });
            c.bind("event", Val::Json { sql: format!("({}::text)::jsonb", marker("__event")), record: Some(format!("event:{event}")) });
            let mut conds = Vec::new();
            // the event fixes the tenant of the endpoints it may reach, whatever the condition says
            if tenant::scoped(self.core, &o.entity)
                && let Some((field, anchor)) = tenant::event_anchor(self.core, event)
            {
                let id = format!("((({}::text)::jsonb ->> {})::uuid)", marker("__event"), lit(&field));
                if let (Some(sel), Some(mine)) =
                    (tn::rows_select(self.core, self.s, &anchor, &id, false), tn::row_sql(self.core, self.s, &o.entity, &a))
                {
                    c.tenant = Some(TenantScope { sql: format!("({sel} LIMIT 1)"), checked: Vec::new() });
                    conds.push(format!("{mine} = ({sel} LIMIT 1)"));
                }
            }
            if let Some(f) = &o.filter {
                conds.push(c.pred(f));
            }
            if t.soft_delete {
                conds.push(format!("{a}.\"deleted_at\" IS NULL"));
            }
            conds.push(format!("{a}.{} IS NOT NULL", q(&url_col)));
            conds.push(format!(
                "NOT EXISTS (SELECT 1 FROM \"_aip_outbound_endpoint\" d WHERE d.\"form\" = {} AND d.\"endpoint\" = {a}.\"id\" AND d.\"disabled_at\" IS NOT NULL)",
                lit(&o.entity)
            ));
            let event_id = format!("('evt_' || {}::text)", marker("__outbox"));
            let body = format!(
                "jsonb_build_object('id', {event_id}, 'type', {}, 'created_at', to_char(now() AT TIME ZONE 'utc', 'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"'), 'data', ({}::text)::jsonb)",
                lit(event),
                marker("__event")
            );
            let text = format!(
                "INSERT INTO \"_aip_outbound_delivery\" (\"form\", \"endpoint\", \"event\", \"event_id\", \"body\", \"deadline\", \"next_attempt_at\") \
                 SELECT {}, {a}.\"id\", {}, {event_id}, {body}, now() + make_interval(secs => {}), now() FROM {} {a} WHERE {} \
                 ON CONFLICT DO NOTHING",
                lit(&o.entity),
                lit(event),
                o.over_seconds,
                q(&t.table),
                conds.join(" AND ")
            );
            c.pop();
            self.absorb(&mut c);
            let step = Step::Exec { sql: super::sql(text), bind: None, label: format!("outbound webhooks {}: queue {event}", o.entity) };
            handlers.push(Handler { event: event.clone(), name: format!("outbound_{}_{event}", o.entity), when: None, steps: vec![step] });
        }
        let plan = Outbound {
            entity: o.entity.clone(),
            table: t.table.clone(),
            url_column: url_col,
            soft_delete: t.soft_delete,
            events: o.events.clone(),
            retry: o.retry.max(0) as u32,
            over_seconds: o.over_seconds.max(1) as u64,
            disable_after_seconds: o.disable_after_seconds.map(|s| s.max(1) as u64),
        };
        (handlers, Some(plan))
    }
}
