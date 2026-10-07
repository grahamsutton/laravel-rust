/// A basic test example.
#[tokio::test]
async fn test_the_application_returns_a_successful_response() {
    let mut app = crate::app();

    app.get("/").await.assert_status(200);
}
