-- Minimal resident behavior for an attended hot-reload check.
-- Restore the original behavior with: push-app.py brain-default
return {
    on_event = function(event)
        -- This demo intentionally holds one expression. Native Error/Panic
        -- reactions and terminal power-off retain their higher priority.
    end,
    tick = function(now_ms)
        fox.set_expression("Happy")
    end,
}
