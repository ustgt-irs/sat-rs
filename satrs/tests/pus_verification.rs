#[cfg(feature = "crossbeam")]
pub mod crossbeam_test {
    use arbitrary_int::traits::Integer as _;
    use arbitrary_int::u14;
    use hashbrown::HashMap;
    use satrs::legacy::pus::test_util::{TEST_APID, TEST_COMPONENT_ID_0};
    use satrs::legacy::pus::verification::{
        FailParams, RequestId, VerificationReporter, VerificationReporterConfig,
        VerificationReportingProvider,
    };
    use satrs::pool::{PoolProvider, PoolProviderWithGuards, StaticMemoryPool, StaticPoolConfig};
    use satrs::tmtc::{PacketSenderWithSharedPool, SharedStaticMemoryPool};
    use spacepackets::SpHeader;
    use spacepackets::ecss::tc::{PusTcCreator, PusTcReader, PusTcSecondaryHeader};
    use spacepackets::ecss::tm::PusTmReader;
    use spacepackets::ecss::{CreatorConfig, EcssEnumU8, EcssEnumU16, MessageTypeId};
    use std::sync::RwLock;
    use std::thread;
    use std::time::Duration;

    const FIXED_STAMP: [u8; 7] = [0; 7];
    const PACKETS_SENT: u8 = 8;

    /// This test also shows how the verification report could be used in a multi-threaded context,
    /// wrapping it into an [Arc] and [Mutex] and then passing it to two threads.
    ///
    ///  - The first thread generates a acceptance, a start, two steps and one completion report
    ///  - The second generates an acceptance and start success report and a completion failure
    ///  - The third thread is the verification receiver. In the test case, it verifies the other two
    ///    threads have sent the correct expected verification reports
    #[test]
    fn test_shared_reporter() {
        // We use a synced sequence count provider here because both verification reporters have the
        // the same APID. If they had distinct APIDs, the more correct approach would be to have
        // each reporter have an own sequence count provider.
        let cfg = VerificationReporterConfig::new(TEST_APID, 1, 2, 8);
        // Shared pool object to store the verification PUS telemetry
        let pool_cfg = StaticPoolConfig::new_from_subpool_cfg_tuples(
            vec![(10, 32), (10, 64), (10, 128), (10, 1024)],
            false,
        );
        let shared_tm_pool =
            SharedStaticMemoryPool::new(RwLock::new(StaticMemoryPool::new(pool_cfg.clone())));
        let shared_tc_pool =
            SharedStaticMemoryPool::new(RwLock::new(StaticMemoryPool::new(pool_cfg)));
        let shared_tc_pool_1 = shared_tc_pool.clone();
        let (tx, rx) = crossbeam_channel::bounded(10);
        let sender =
            PacketSenderWithSharedPool::new_with_shared_packet_pool(tx.clone(), &shared_tm_pool);
        let sender_1 = sender.clone();
        let reporter_with_sender_0 = VerificationReporter::new(TEST_COMPONENT_ID_0.id(), &cfg);
        let reporter_with_sender_1 = reporter_with_sender_0.clone();
        // For test purposes, we retrieve the request ID from the TCs and pass them to the receiver
        // tread.
        let req_id_0;
        let req_id_1;

        let (tx_tc_0, rx_tc_0) = crossbeam_channel::bounded(3);
        let (tx_tc_1, rx_tc_1) = crossbeam_channel::bounded(3);
        {
            let mut tc_guard = shared_tc_pool.write().unwrap();
            let sph = SpHeader::new_for_unseg_tc(TEST_APID, u14::ZERO, 0);
            let tc_header = PusTcSecondaryHeader::new_simple(MessageTypeId::new(17, 1));
            let pus_tc_0 = PusTcCreator::new_no_app_data(sph, tc_header, CreatorConfig::default());
            req_id_0 = RequestId::new(&pus_tc_0);
            let addr = tc_guard
                .free_element(pus_tc_0.len_written(), |buf| {
                    pus_tc_0.write_to_slice(buf).unwrap();
                })
                .unwrap();
            tx_tc_0.send(addr).unwrap();
            let sph = SpHeader::new_for_unseg_tc(TEST_APID, u14::new(1), 0);
            let tc_header = PusTcSecondaryHeader::new_simple(MessageTypeId::new(5, 1));
            let pus_tc_1 = PusTcCreator::new_no_app_data(sph, tc_header, CreatorConfig::default());
            req_id_1 = RequestId::new(&pus_tc_1);
            let addr = tc_guard
                .free_element(pus_tc_0.len_written(), |buf| {
                    pus_tc_1.write_to_slice(buf).unwrap();
                })
                .unwrap();
            tx_tc_1.send(addr).unwrap();
        }
        let verif_sender_0 = thread::spawn(move || {
            let mut tc_buf: [u8; 1024] = [0; 1024];
            let tc_addr = rx_tc_0
                .recv_timeout(Duration::from_millis(20))
                .expect("Receive timeout");
            let tc_len;
            {
                let mut tc_guard = shared_tc_pool.write().unwrap();
                let pg = tc_guard.read_with_guard(tc_addr);
                tc_len = pg.read(&mut tc_buf).unwrap();
            }
            let _tc = PusTcReader::new(&tc_buf[0..tc_len]).unwrap();

            let token = reporter_with_sender_0.start_verification_with_req_id(req_id_0);
            let accepted_token = reporter_with_sender_0
                .acceptance_success(&sender, token, &FIXED_STAMP)
                .expect("Acceptance success failed");

            // Do some start handling here
            let started_token = reporter_with_sender_0
                .start_success(&sender, accepted_token, &FIXED_STAMP)
                .expect("Start success failed");
            // Do some step handling here
            reporter_with_sender_0
                .step_success(&sender, &started_token, &FIXED_STAMP, EcssEnumU8::new(0))
                .expect("Start success failed");

            // Finish up
            reporter_with_sender_0
                .step_success(&sender, &started_token, &FIXED_STAMP, EcssEnumU8::new(1))
                .expect("Start success failed");
            reporter_with_sender_0
                .completion_success(&sender, started_token, &FIXED_STAMP)
                .expect("Completion success failed");
        });

        let verif_sender_1 = thread::spawn(move || {
            let mut tc_buf: [u8; 1024] = [0; 1024];
            let tc_addr = rx_tc_1
                .recv_timeout(Duration::from_millis(20))
                .expect("Receive timeout");
            let tc_len;
            {
                let mut tc_guard = shared_tc_pool_1.write().unwrap();
                let pg = tc_guard.read_with_guard(tc_addr);
                tc_len = pg.read(&mut tc_buf).unwrap();
            }
            let tc = PusTcReader::new(&tc_buf[0..tc_len]).unwrap();
            let token = reporter_with_sender_1.start_verification(&tc);
            let accepted_token = reporter_with_sender_1
                .acceptance_success(&sender_1, token, &FIXED_STAMP)
                .expect("Acceptance success failed");
            let started_token = reporter_with_sender_1
                .start_success(&sender_1, accepted_token, &FIXED_STAMP)
                .expect("Start success failed");
            let fail_code = EcssEnumU16::new(2);
            let params = FailParams::new_no_fail_data(&FIXED_STAMP, &fail_code);
            reporter_with_sender_1
                .completion_failure(&sender_1, started_token, params)
                .expect("Completion success failed");
        });

        let verif_receiver = thread::spawn(move || {
            let mut packet_counter = 0;
            let mut tm_buf: [u8; 1024] = [0; 1024];
            let mut verif_map = HashMap::new();
            while packet_counter < PACKETS_SENT {
                let tm_in_pool = rx
                    .recv_timeout(Duration::from_millis(50))
                    .expect("Packet reception timeout");
                let tm_len;
                {
                    let mut rg = shared_tm_pool.write().expect("Error locking shared pool");
                    let store_guard = rg.read_with_guard(tm_in_pool.store_addr);
                    tm_len = store_guard
                        .read(&mut tm_buf)
                        .expect("Error reading TM slice");
                }
                let pus_tm =
                    PusTmReader::new(&tm_buf[0..tm_len], 7).expect("Error reading verification TM");
                let req_id =
                    RequestId::from_bytes(&pus_tm.source_data()[0..RequestId::SIZE_AS_BYTES])
                        .expect("reading request ID from PUS TM source data failed");
                if !verif_map.contains_key(&req_id) {
                    let content = vec![pus_tm.service_type_id()];
                    verif_map.insert(req_id, content);
                } else {
                    let content = verif_map.get_mut(&req_id).unwrap();
                    content.push(pus_tm.message_subtype_id())
                }
                packet_counter += 1;
            }
            for (req_id, content) in verif_map {
                if req_id == req_id_1 {
                    assert_eq!(content[0], 1);
                    assert_eq!(content[1], 3);
                    assert_eq!(content[2], 8);
                } else if req_id == req_id_0 {
                    assert_eq!(content[0], 1);
                    assert_eq!(content[1], 3);
                    assert_eq!(content[2], 5);
                    assert_eq!(content[3], 5);
                    assert_eq!(content[4], 7);
                } else {
                    panic!("Unexpected request ID {:?}", req_id);
                }
            }
        });
        verif_sender_0.join().expect("Joining thread 0 failed");
        verif_sender_1.join().expect("Joining thread 1 failed");
        verif_receiver.join().expect("Joining thread 2 failed");
    }
}
