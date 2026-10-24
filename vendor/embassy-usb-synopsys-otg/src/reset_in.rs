//! Non-EP0 IN reset shutdown: NAK effective, then disable, then inactive.
//! The caller polls once and yields while hardware advances; no timeouts or spins.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Phase {
    Idle,
    Nak,
    Disable,
}
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Action {
    Done,
    Nak,
    Disable,
    Wait,
}
pub(crate) fn step(phase: &mut Phase, enabled: bool, nak_effective: bool, disabled_ack: bool) -> Action {
    if *phase == Phase::Disable {
        if !enabled && disabled_ack {
            *phase = Phase::Idle;
            return Action::Done;
        }
        return Action::Wait;
    }
    if !enabled {
        *phase = Phase::Idle;
        return Action::Done;
    }
    match phase {
        Phase::Idle => {
            *phase = Phase::Nak;
            Action::Nak
        }
        Phase::Nak if nak_effective => {
            *phase = Phase::Disable;
            Action::Disable
        }
        Phase::Nak | Phase::Disable => Action::Wait,
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn active_transfer_cannot_reconfigure_before_nak_and_disable_complete() {
        let mut phase = Phase::Idle;
        assert_eq!(step(&mut phase, true, false, false), Action::Nak);
        for _ in 0..10 {
            assert_eq!(step(&mut phase, true, false, false), Action::Wait);
        }
        assert_eq!(step(&mut phase, true, true, false), Action::Disable);
        for _ in 0..10 {
            assert_eq!(step(&mut phase, true, true, false), Action::Wait);
        }
        assert_eq!(step(&mut phase, false, true, false), Action::Wait);
        assert_eq!(step(&mut phase, false, true, true), Action::Done);
        assert_eq!(phase, Phase::Idle);
    }
    #[test]
    fn inactive_and_naturally_completed_transfers_need_no_disable_command() {
        let mut phase = Phase::Idle;
        assert_eq!(step(&mut phase, false, false, false), Action::Done);
        assert_eq!(step(&mut phase, true, true, false), Action::Nak);
        assert_eq!(step(&mut phase, false, true, false), Action::Done);
        assert_eq!(step(&mut phase, true, false, false), Action::Nak);
    }
}
