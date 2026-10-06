WITH objects AS (
    SELECT c.* FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace
    WHERE n.nspname=$1 AND c.relkind NOT IN ('i','I')
)
SELECT jsonb_build_object(
    'relations', COALESCE((SELECT jsonb_agg(jsonb_build_array(
        o.relname,o.relkind,o.relpersistence,o.relrowsecurity,o.relforcerowsecurity,
        COALESCE((SELECT jsonb_agg(jsonb_build_array(
            a.attname,a.attnum,format_type(a.atttypid,a.atttypmod),a.attnotnull,
            pg_get_expr(d.adbin,d.adrelid,false),a.attidentity,a.attgenerated,
            cn.nspname,co.collname
        ) ORDER BY a.attnum)
        FROM pg_attribute a LEFT JOIN pg_attrdef d ON d.adrelid=a.attrelid AND d.adnum=a.attnum
        LEFT JOIN pg_collation co ON co.oid=a.attcollation LEFT JOIN pg_namespace cn ON cn.oid=co.collnamespace
        WHERE a.attrelid=o.oid AND a.attnum>0 AND NOT a.attisdropped),'[]'::jsonb)
    ) ORDER BY o.relname) FROM objects o),'[]'::jsonb),
    'constraints', COALESCE((SELECT jsonb_agg(jsonb_build_array(
        o.relname,c.conname,c.contype,c.condeferrable,c.condeferred,c.convalidated,
        c.connoinherit,pg_get_constraintdef(c.oid,false)
    ) ORDER BY o.relname,c.conname) FROM pg_constraint c JOIN objects o ON o.oid=c.conrelid),'[]'::jsonb),
    'indexes', COALESCE((SELECT jsonb_agg(jsonb_build_array(
        o.relname,c.relname,pg_get_indexdef(i.indexrelid,0,false),
        i.indisunique,i.indisprimary,i.indisexclusion,i.indimmediate,i.indisvalid,i.indisready,i.indislive
    ) ORDER BY o.relname,c.relname) FROM pg_index i JOIN objects o ON o.oid=i.indrelid
    JOIN pg_class c ON c.oid=i.indexrelid),'[]'::jsonb),
    'sequences', COALESCE((SELECT jsonb_agg(jsonb_build_array(
        o.relname,format_type(s.seqtypid,NULL),s.seqstart,s.seqincrement,s.seqmax,s.seqmin,s.seqcache,s.seqcycle,
        COALESCE((SELECT jsonb_agg(jsonb_build_array(n.nspname,t.relname,a.attname,d.deptype) ORDER BY n.nspname,t.relname,a.attname,d.deptype)
            FROM pg_depend d JOIN pg_class t ON t.oid=d.refobjid
            JOIN pg_namespace n ON n.oid=t.relnamespace
            JOIN pg_attribute a ON a.attrelid=t.oid AND a.attnum=d.refobjsubid
            WHERE d.classid='pg_class'::regclass AND d.objid=o.oid AND d.refclassid='pg_class'::regclass
            AND d.deptype IN ('a','i')),'[]'::jsonb)
    ) ORDER BY o.relname) FROM pg_sequence s JOIN objects o ON o.oid=s.seqrelid),'[]'::jsonb),
    'triggers', COALESCE((SELECT jsonb_agg(jsonb_build_array(
        o.relname,t.tgname,t.tgenabled,pg_get_triggerdef(t.oid,false)
    ) ORDER BY o.relname,t.tgname) FROM pg_trigger t JOIN objects o ON o.oid=t.tgrelid WHERE NOT t.tgisinternal),'[]'::jsonb),
    'rules', COALESCE((SELECT jsonb_agg(jsonb_build_array(
        o.relname,r.rulename,r.ev_enabled,pg_get_ruledef(r.oid,false)
    ) ORDER BY o.relname,r.rulename) FROM pg_rewrite r JOIN objects o ON o.oid=r.ev_class),'[]'::jsonb),
    'policies', COALESCE((SELECT jsonb_agg(jsonb_build_array(
        o.relname,p.polname,p.polcmd,p.polpermissive,
        (SELECT jsonb_agg(CASE WHEN id=0 THEN 'public' ELSE pg_get_userbyid(id)::text END ORDER BY CASE WHEN id=0 THEN 'public' ELSE pg_get_userbyid(id)::text END) FROM unnest(p.polroles) id),
        pg_get_expr(p.polqual,p.polrelid,false),pg_get_expr(p.polwithcheck,p.polrelid,false)
    ) ORDER BY o.relname,p.polname) FROM pg_policy p JOIN objects o ON o.oid=p.polrelid),'[]'::jsonb),
    'inheritance', COALESCE((SELECT jsonb_agg(jsonb_build_array(
        cn.nspname,c.relname,pn.nspname,p.relname,i.inhseqno
    ) ORDER BY cn.nspname,c.relname,i.inhseqno) FROM pg_inherits i
    JOIN pg_class c ON c.oid=i.inhrelid JOIN pg_namespace cn ON cn.oid=c.relnamespace
    JOIN pg_class p ON p.oid=i.inhparent JOIN pg_namespace pn ON pn.oid=p.relnamespace
    WHERE cn.nspname=$1 OR pn.nspname=$1),'[]'::jsonb)
)::text
