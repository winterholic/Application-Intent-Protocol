//! `webhook X via source(options) { on "event"(e: T) do { ... } }` lowers to a
//! `Webhook`: verification settings resolved from the source, and one step list
//! per event with the payload bound as JSON (typed by the record, if given).

use super::Planner;
use crate::sqlexpr::{Val, marker};
use crate::ty::Ty;
use aip_ir as ir;
use aip_plan::*;

impl Planner<'_> {
    pub fn webhook(&mut self, w: &ir::Webhook) -> Webhook {
        let st = ir::builtin::webhook_settings(w);
        let mut handlers = Vec::new();
        for h in &w.handlers {
            let mut c = self.compiler();
            c.push();
            let record = match h.ty.as_ref().map(Ty::from_ir) {
                Some(Ty::Record(r)) => Some(r),
                _ => None,
            };
            c.bind(&h.binding, Val::Json { sql: format!("({}::text)::jsonb", marker(&h.binding)), record });
            // no row names a tenant before the payload is read: the first write fixes it, `cross tenant` lifts that
            let mut steps: Vec<Step> = self.context_pin(&c, h.cross_tenant).into_iter().collect();
            self.stmts(&mut c, &h.body, &mut steps);
            c.pop();
            self.absorb(&mut c);
            handlers.push(WebhookHandler { event: h.event.clone(), binding: h.binding.clone(), steps });
        }
        Webhook {
            name: w.name.clone(),
            source: st.source,
            scheme: st.scheme,
            header: st.header,
            secret_env: st.secret_env,
            event_path: st.event_path,
            id_path: st.id_path,
            payload_path: st.payload_path,
            handlers,
        }
    }
}
