#[test]
fn an_identity_pose_produces_exactly_the_old_pixels() {
    use grok_ball::raster::*;
    use grok_ball::skin;
    use grok_ball::engine::BallEngine;
    use grok_ball::types::BallExpression;

    for expression in BallExpression::ALL {
        let mut engine = BallEngine::new();
        engine.set_expression(expression);
        let frame = engine.advance(400);
        let mut plain = [0u16; 240 * 240];
        let mut posed = [0u16; 240 * 240];
        render_sized(&frame, &mut plain, 240);
        render_sized_with_pose_and_skin(
            &frame, &mut posed, 240, Quality::TwoByTwo, CanvasPose::IDENTITY, &skin::BLOB,
        );
        assert_eq!(plain, posed, "identity pose diverged for {expression:?}");
    }
}
