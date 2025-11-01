#[cfg(test)]
mod test {
    #[tokio::test]
    async fn test_row_is_clone() {
        let (client, connection) = tokio_postgres::connect(
            "host=localhost user=postgres dbname=tamanu_meta",
            tokio_postgres::NoTls,
        ).await.unwrap();

        tokio::spawn(async move {
            let _ = connection.await;
        });

        let rows = client.query("SELECT 1", &[]).await.unwrap();
        let row1 = &rows[0];
        let row2 = row1.clone(); // This will fail if Row doesn't implement Clone
        drop(row2);
    }
}
