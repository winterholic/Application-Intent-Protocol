# FlaskBB upstream plugins captured from the baseline runtime

The frozen FlaskBB project declares these plugin distributions as dependencies. We preserve the exact installed Python modules/templates/migration and wheel metadata used for the original baseline, because their routes and tables are part of the porting denominator.

| Distribution | Version | Metadata license | Metadata author | Provenance |
|---|---:|---|---|---|
| flaskbb-plugin-conversations | 1.0.6 | BSD License | Peter Justin | Exact installed distribution metadata in `conversations/dist-info/`; Home-page https://flaskbb.org |
| flaskbb-plugin-portal | 1.1.3 | BSD | FlaskBB Team | Exact installed distribution metadata in `portal/dist-info/`; Home-page https://flaskbb.org |

The package metadata does not include a separate LICENSE text file. Keep the upstream author's copyright/license notices in module headers. Distribution versions are pinned in the reproducibility record. The source modules are preserved for inspection only and are not modified by the AIP port.
