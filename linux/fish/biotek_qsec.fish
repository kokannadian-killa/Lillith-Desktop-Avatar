# Biotek / QuantumSec Fish helpers (additive — source from config.fish).
# Never enables systemd. Never runs lockdown commands.

function biotek_home
    if set -q BIOTEK_HOME
        echo $BIOTEK_HOME
    else
        echo "$HOME/.biotek"
    end
end

function biotek_qsec_status
    set -l p (biotek_home)/quantumsec/status.json
    if test -f $p
        cat $p
    else
        echo "QuantumSec: no status yet (start Lillith or: lillith quantumsec)"
    end
end

function biotek_qsec_reports
    ls -1 (biotek_home)/reports/personalization-*.md 2>/dev/null
end

function biotek_qsec_consent_tail
    set -l d (biotek_home)/consent
    set -l f (ls -1 $d/consent-*.jsonl 2>/dev/null | tail -n 1)
    if test -n "$f"
        tail -n 20 $f
    else
        echo "no consent log yet"
    end
end

# Optional: alias for DemonOS / PhantomCore branding familiarity
function phantom_qsec_status
    biotek_qsec_status
end
