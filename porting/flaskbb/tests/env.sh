#!/bin/sh
# Isolated FlaskBB port fixtures (never the shared 5432). Override the ports through
# the environment when another session already holds these.
: "${FLASKBB_FIX_PG_PORT:=55471}"
export AIP_FLASKBB_TEST_DATABASE_URL="postgresql://aip_flaskbb:aip_flaskbb_fixture_password@127.0.0.1:${FLASKBB_FIX_PG_PORT}/aip_flaskbb"
export AIP_TEST_DATABASE_URL="$AIP_FLASKBB_TEST_DATABASE_URL"
